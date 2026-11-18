import sys

DO_REQUEST = 0.15e-6
DO_GBS = 12.50e-6
DO_GB = 0.128
ROW_WRITE = 1.00e-6
R2_PUT = 4.50e-6
WORKER_REQUEST = 0.30e-6
QUEUE_OP = 0.40e-6
MONTH = 30 * 24 * 3600
HOLD_TIMEOUT_S = 5
WAL_WINDOW_S = 60


def active_seconds(rate, busy_ms):
    return min(MONTH, rate * MONTH * busy_ms / 1000)


def candidates(rate):
    requests = rate * MONTH
    flushes_hold = min(requests, MONTH / HOLD_TIMEOUT_S) if rate else 0
    flushes_wal = min(requests, MONTH / WAL_WINDOW_S) if rate else 0
    worker = requests * WORKER_REQUEST
    return {
        "hold (ack after R2 commit, no WAL)": worker
        + requests * DO_REQUEST
        + flushes_hold * (R2_PUT + ROW_WRITE)
        + min(MONTH, flushes_hold * HOLD_TIMEOUT_S) * DO_GB * DO_GBS,
        "wal (ack after 1 SQLite row, alarm flush)": worker
        + requests * DO_REQUEST
        + requests * 2 * ROW_WRITE
        + flushes_wal * (R2_PUT + ROW_WRITE + DO_REQUEST)
        + active_seconds(rate, 5) * DO_GB * DO_GBS,
        "queue (ack after enqueue, batch consumer)": worker
        + requests * 3 * QUEUE_OP
        + flushes_wal * (R2_PUT + ROW_WRITE + DO_REQUEST),
        "per-request object (ack after R2 put)": worker
        + requests * (R2_PUT + DO_REQUEST + ROW_WRITE)
        + active_seconds(rate, 50) * DO_GB * DO_GBS,
    }


rates = [float(r) for r in sys.argv[1:]] or [0.01, 0.1, 1, 10, 100]
print("USD per ingest cell per month, list prices, before included allowances")
print(f"{'req/s':>8} " + " ".join(f"{name[:26]:>28}" for name in candidates(1)))
for rate in rates:
    row = candidates(rate)
    print(f"{rate:>8} " + " ".join(f"{cost:>28.2f}" for cost in row.values()))
