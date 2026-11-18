# Agent deployment threat model

## Boundary

One deployment is one customer security domain. It contains one S3
bucket, one celld fleet bucket, one deployed world-celld worker, and one Eve
0.51 runtime. World-celld is experimental. Celld does not isolate hostile
applications, so separate customers need separate fleets and
operating-system boundaries.

The agent container supervises exactly two long-lived processes: celld and
Eve. Celld and Eve listen on container loopback only. The host publishes
their ports on loopback for trusted operators and SSH tunnels.

Each job's repository sandbox runs on Ascii Box, a hosted service outside
this container. The trusted agent host talks to Box over its API and holds
the Box API key; a sandbox never receives that key.

## Protected values

- Customer source code and patches.
- Claude or Codex subscription state.
- AWS credentials and durable workflow state in S3.
- Persistent world and workflow callback secrets.
- Telemetry evidence and publication receipts.
- The host GitHub credential.

## Controls

| Threat | Control |
|---|---|
| Repository text requests secrets | Provider clients receive only the variables required for the run. World, callback, AWS, Box account, and host GitHub credentials are not sandbox inputs. `noEnv` is fixed on for every box, so a box starts with none of the operator's Box account secrets, secret files, GitHub credentials, SSH identity, or selected repositories. |
| Unauthorized workflow RPC | The generated world secret is required by world-celld. It is stored outside the image and persisted with agent state. |
| Forged workflow callback | A separate callback secret is generated and supplied to the deployed worker, which forwards it as `x-workflow-callback-secret`. Eve 0.51 has no proven callback-secret environment variable, so no unsupported Eve setting is used. |
| Public celld administration | Celld and its internal listener bind to container loopback; only the selected host celld port is mapped to host loopback. |
| Model edits outside policy | The Eve execution and publication path checks allowed paths, base SHA, required tests, and Git state before publishing. This check runs on the trusted host, not inside the sandbox. |
| False publication success | A completed turn or status response is not sufficient. Publication requires a durable receipt naming the tested commit and pull request. Publication opens a draft pull request only; nothing merges automatically. |
| Lost or stale durable work | S3 is the celld persistence backend. The worker uses pinned deployment state and the configured 90-day maximum workflow age. |
| Mutable deployment inputs | Pin the celld v0.4.1 binary checksum and base image digests. Pin Eve and world-celld versions in the lockfile and prove their compatibility in integration. |

## The sandbox trust boundary changed

Sandboxes now run on Ascii Box instead of firewalled Docker containers on the
trusted host. This is a real reduction in what the sandbox boundary
enforces, and this section states it plainly rather than qualifying it away.

**What is gone.** The previous deployment ran each job in a Docker container
with a deny-all egress network policy: the sandbox could reach nothing on
the network. Box does not support Eve's fine-grained network policies, so
that control is gone, and the deployment now accepts the resulting risk
instead of pretending it still holds. Model-controlled code running in a box
can make arbitrary outbound network calls, including calls that exfiltrate
any repository contents the box can read. Treat every job's box as capable of
sending its checked-out workspace, or anything else written into it, to a
destination the model chooses.

**What still holds.**

- The host GitHub credential never enters a box. `gh` authentication lives
  only on the trusted host, and the box is created with `noEnv`, so it never
  receives the operator's GitHub credential, SSH identity, secret files, or
  selected repositories.
- The workspace arrives in the box as a credential-free git bundle. The
  trusted host clones the repository, checks out the job's exact
  `baseSha`, bundles that single ref, and writes the bundle into the box.
  The bundle carries no remote and no credential, so a box that reads its
  own workspace cannot use it to authenticate back to GitHub as the
  operator.
- The allowed-path policy is enforced on the trusted host. The host filters
  the staged file list against the job's allowed paths and its own forbidden
  list, then commits blobs only for paths that survive. A box cannot land a
  file outside the allowlist by hiding it from the diff, because the host
  commits only what it filtered.
- Publication is a draft pull request. Nothing the box does merges code.

The required tests are a weaker control than the two above, and this document
does not count them as a boundary. The host runs each declared test command
inside the box and refuses to publish unless every one exits zero, but it
reads the box's own exit code. It cannot prove the command ran. Model-authored
code that shadows a test binary earlier in `PATH` exits zero and the host
believes it. Read the required tests as a check against an honest mistake, not
against a box that chooses to defeat its own gate. This limit predates Box and
is not caused by it; the Docker backend read exit codes the same way.

**Accepted risk.** A model-controlled box can read and exfiltrate any
repository content it can check out, and Box cannot stop that egress. The
deployment accepts this risk because the box never holds a credential that
would let exfiltrated content be used to act as the operator, and because the
path policy is enforced on the host and publication stops at a draft pull
request, so exfiltration does not translate into unauthorized changes landing
in the repository. The argument rests on the path policy and the draft gate,
not on the required tests.

**What would reverse this.** Restoring an enforced egress policy, whether
through a Box-side network control or a different sandbox backend that
supports one, would remove the accepted risk above. Until then, do not run
this deployment against a repository whose contents must never leave the
operator's network.

## Known limitations

- S3 is not made redundant by this deployment. Use the durability and
  failover properties of the bucket you provision, and back it up
  consistently with `agent-data`.
- World-celld and celld are experimental or alpha components. A package build
  mismatch is not, by itself, evidence of a runtime incompatibility.
- Shared hosted tenancy, automatic triggering, multi-worker failover, and a
  pre-installed sandbox dependency image are not provided by this deployment.
  A cold box carries language runtimes but not the target repository's
  packages, so a job declares its own `setup` commands and the trusted host
  runs them after checkout. They come from the control plane with the job,
  never from the model, and a failure aborts preparation.
- Boxes run only in the EU. Job data and box snapshots stay in Box's EU
  regions; this is a deployment constraint, not a configurable choice.
- Box has no compliance certification yet; SOC 2 is in progress. Do not
  claim a certification this deployment does not have.
- The adapter does not fork prepared template boxes, so per-session setup
  replays on every new box rather than cloning a prepared snapshot. Upstream
  issue [ariana-dot-dev/eve-box#1](https://github.com/ariana-dot-dev/eve-box/issues/1)
  tracks it.
- Customer GitHub and subscription-provider login remain required. Provider
  policy and subscription access can change; the deployment does not broker
  consumer OAuth.
- Exact direct job request fields and cancellation route names remain subject
  to the source-side world-celld API contract. Integration must verify them
  before documenting a stable client SDK.
