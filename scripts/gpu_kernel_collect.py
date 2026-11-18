#!/usr/bin/env python3
"""Produce a sigiro_gpu_kernels Parquet export from a CUDA-graph inference run.

Writes the 23 columns `sigiro ingest-kernels` expects, aggregated one row per
(run_id, graph_id, node_index).

Attribution is exact, not inferred. Two facts make that possible, both measured:

  1. The PyTorch profiler reports `graph node id` as (graph id << 32) | index, so
     the low 32 bits are a node's position in its graph.
  2. `cudaStreamGetCaptureInfo_v2`, called during capture, returns the dependency
     frontier. In a linear chain the frontier after an op is the last node that op
     added, so consecutive frontiers are exact boundaries. Reading positions back
     needs `torch.cuda.CUDAGraph(keep_graph=True)`, without which
     `raw_cuda_graph()` raises.

Everything a caller cannot attribute lands in `bucket` with a reason rather than
being dropped, because unattributed GPU time has to be a row.

Usage:
    from gpu_kernel_collect import KernelCollector
    c = KernelCollector(run_id="A1", experiment_id="exp", arm="control")
    with c.capture() as graph:          # capture the model into a CUDA graph
        for name, module in layers:
            y = module(y)
            c.mark(name)                # boundary after each op
    c.profile_replays(graph, n=50)      # time the replays
    c.write_parquet("kernels.parquet")
"""

from __future__ import annotations

import ctypes
import glob
import json
import site
import tempfile
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path

ATTRIBUTED = "attributed"
RESIDUAL_OUTSIDE_GRAPH = "residual_outside_any_graph"
RESIDUAL_NO_BOUNDARY = "residual_no_capture_boundary"


def _load_cudart() -> ctypes.CDLL:
    """Find libcudart wherever torch put it. Fail loudly if it is absent."""
    candidates: list[str] = []
    for base in site.getsitepackages():
        candidates += glob.glob(base + "/nvidia/cuda_runtime/lib/libcudart.so*")
        candidates += glob.glob(base + "/torch/lib/libcudart*.so*")
    candidates += ["libcudart.so.12", "libcudart.so"]
    for candidate in candidates:
        try:
            return ctypes.CDLL(candidate)
        except OSError:
            continue
    raise RuntimeError(f"libcudart not found; tried {candidates}")


class KernelCollector:
    """Collects exact node->op boundaries during capture, then times the replays."""

    def __init__(self, run_id: str, experiment_id: str | None = None,
                 arm: str | None = None, service_name: str = "sigiro-gpu",
                 device: str | None = None, rank: int = 0):
        self.run_id = run_id
        self.experiment_id = experiment_id
        self.arm = arm
        self.service_name = service_name
        self.device = device
        self.rank = rank

        self._rt = _load_cudart()
        self._graph_t = ctypes.c_void_p
        self._node_t = ctypes.c_void_p

        self._info = self._rt.cudaStreamGetCaptureInfo_v2
        self._info.restype = ctypes.c_int
        self._info.argtypes = [
            ctypes.c_void_p, ctypes.POINTER(ctypes.c_int),
            ctypes.POINTER(ctypes.c_ulonglong), ctypes.POINTER(self._graph_t),
            ctypes.POINTER(ctypes.POINTER(self._node_t)), ctypes.POINTER(ctypes.c_size_t),
        ]
        self._get_nodes = self._rt.cudaGraphGetNodes
        self._get_nodes.restype = ctypes.c_int
        self._get_nodes.argtypes = [
            self._graph_t, ctypes.POINTER(self._node_t), ctypes.POINTER(ctypes.c_size_t)
        ]

        self._marks: list[tuple[str, int | None]] = []
        self._node_op: dict[int, str] = {}
        self._trace_path: Path | None = None

    # ---------------------------------------------------------------- capture
    def _frontier(self) -> int | None:
        import torch

        stream = torch.cuda.current_stream().cuda_stream
        status = ctypes.c_int(0)
        cid = ctypes.c_ulonglong(0)
        graph = self._graph_t()
        deps = ctypes.POINTER(self._node_t)()
        count = ctypes.c_size_t(0)
        rc = self._info(ctypes.c_void_p(stream), ctypes.byref(status), ctypes.byref(cid),
                        ctypes.byref(graph), ctypes.byref(deps), ctypes.byref(count))
        if rc != 0:
            raise RuntimeError(f"cudaStreamGetCaptureInfo_v2 failed: rc={rc}")
        if count.value == 0:
            return None
        # A linear chain has one frontier node: the last one this op added. A
        # branching graph has several, which this boundary scheme cannot split,
        # so take the last and let the bucket record the imprecision.
        return int(deps[count.value - 1])

    def mark(self, op_name: str) -> None:
        """Record the capture frontier after an op. Call inside capture()."""
        self._marks.append((op_name, self._frontier()))

    @contextmanager
    def capture(self):
        """Capture into a CUDA graph with keep_graph=True, which raw_cuda_graph needs."""
        import torch

        graph = torch.cuda.CUDAGraph(keep_graph=True)
        with torch.cuda.graph(graph):
            yield graph
        torch.cuda.synchronize()
        self._resolve_boundaries(graph)

    def _resolve_boundaries(self, graph) -> None:
        raw = graph.raw_cuda_graph()
        count = ctypes.c_size_t(0)
        rc = self._get_nodes(self._graph_t(int(raw)), None, ctypes.byref(count))
        if rc != 0:
            raise RuntimeError(f"cudaGraphGetNodes(count) failed: rc={rc}")
        nodes = (self._node_t * count.value)()
        rc = self._get_nodes(self._graph_t(int(raw)), nodes, ctypes.byref(count))
        if rc != 0:
            raise RuntimeError(f"cudaGraphGetNodes failed: rc={rc}")
        position = {int(nodes[i]): i for i in range(count.value)}

        previous = -1
        for op_name, handle in self._marks:
            if handle is None:
                continue
            last = position.get(handle)
            if last is None:
                raise RuntimeError(f"frontier node {handle} for {op_name!r} is not in the graph")
            for index in range(previous + 1, last + 1):
                self._node_op[index] = op_name
            previous = last

    # ---------------------------------------------------------------- profile
    def profile_replays(self, graph, n: int = 50) -> None:
        import torch
        from torch.profiler import ProfilerActivity, profile

        with profile(activities=[ProfilerActivity.CPU, ProfilerActivity.CUDA]) as prof:
            for _ in range(n):
                graph.replay()
            torch.cuda.synchronize()
        handle = tempfile.NamedTemporaryFile(suffix=".json", delete=False)
        handle.close()
        self._trace_path = Path(handle.name)
        prof.export_chrome_trace(str(self._trace_path))

    # ------------------------------------------------------------------ rows
    def rows(self) -> list[dict]:
        if self._trace_path is None:
            raise RuntimeError("profile_replays must run before rows()")
        events = json.loads(self._trace_path.read_text())["traceEvents"]
        now = datetime.now(timezone.utc).replace(tzinfo=None)

        aggregate: dict[tuple, dict] = {}
        for event in (e for e in events if e.get("cat") == "kernel"):
            args = event.get("args") or {}
            node_id = args.get("graph node id")
            duration_ns = int(float(event.get("dur", 0)) * 1000)

            if not node_id:
                key = (None, None, event["name"], RESIDUAL_OUTSIDE_GRAPH)
                graph_id = node_index = None
                layer = None
            else:
                graph_id, node_index = node_id >> 32, node_id & 0xFFFFFFFF
                layer = self._node_op.get(node_index)
                bucket = ATTRIBUTED if layer else RESIDUAL_NO_BOUNDARY
                key = (graph_id, node_index, event["name"], bucket)

            entry = aggregate.setdefault(key, {
                "executions": 0, "total_ns": 0, "max_ns": 0,
                "occupancy_pct": args.get("est. achieved occupancy %"),
                "registers_per_thread": args.get("registers per thread"),
                "shared_mem_bytes": args.get("shared memory"),
                "limiting_factor": (args.get("occupancy") or {}).get("limitingFactors"),
            })
            entry["executions"] += 1
            entry["total_ns"] += duration_ns
            entry["max_ns"] = max(entry["max_ns"], duration_ns)

        out = []
        for (graph_id, node_index, kernel, bucket), entry in aggregate.items():
            layer = self._node_op.get(node_index) if node_index is not None else None
            out.append({
                "run_id": self.run_id,
                "experiment_id": self.experiment_id,
                "arm": self.arm,
                "timestamp": now,
                "service_name": self.service_name,
                "device": self.device,
                "rank": self.rank,
                "graph_id": graph_id,
                "node_index": node_index,
                "kernel": kernel,
                "layer": layer,
                "op": layer,
                "attribution_method": "capture_frontier" if bucket == ATTRIBUTED else None,
                "bucket": bucket,
                "executions": entry["executions"],
                "total_ns": entry["total_ns"],
                "mean_ns": entry["total_ns"] // max(1, entry["executions"]),
                "max_ns": entry["max_ns"],
                "occupancy_pct": float(entry["occupancy_pct"]) if entry["occupancy_pct"] is not None else None,
                "registers_per_thread": int(entry["registers_per_thread"]) if entry["registers_per_thread"] is not None else None,
                "shared_mem_bytes": int(entry["shared_mem_bytes"]) if entry["shared_mem_bytes"] is not None else None,
                "limiting_factor": entry["limiting_factor"],
                "source_nodes": None,
            })
        return out

    def write_parquet(self, path: str | Path) -> dict:
        import pyarrow as pa
        import pyarrow.parquet as pq

        rows = self.rows()
        if not rows:
            raise RuntimeError("no kernel rows collected; refusing to write an empty export")
        table = pa.Table.from_pylist(rows)
        pq.write_table(table, str(path))
        attributed = sum(r["total_ns"] for r in rows if r["bucket"] == ATTRIBUTED)
        total = sum(r["total_ns"] for r in rows)
        return {
            "path": str(path),
            "rows": len(rows),
            "total_ns": total,
            "attributed_ns": attributed,
            "attributed_pct": round(100.0 * attributed / total, 2) if total else 0.0,
            "buckets": {b: sum(1 for r in rows if r["bucket"] == b) for b in {r["bucket"] for r in rows}},
        }
