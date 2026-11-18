---------------------------- MODULE IngestCell ----------------------------
EXTENDS Naturals, FiniteSets

CONSTANTS Requests, MaxObjects, AckMode, OrphanGuard

ASSUME AckMode \in {"hold", "wal", "fast"}
ASSUME OrphanGuard \in BOOLEAN

ObjectIds == 1..MaxObjects
None == 0

VARIABLES
  outstanding,
  acked,
  buffer,
  wal,
  objects,
  manifest,
  pending,
  writing,
  compacting,
  nextId

vars == <<outstanding, acked, buffer, wal, objects, manifest, pending, writing, compacting, nextId>>

RowsOf(ids) == UNION {objects[i] : i \in ids \cap DOMAIN objects}

Queryable == RowsOf(manifest)

Durable == Queryable \cup (IF AckMode = "wal" THEN wal ELSE {})

Init ==
  /\ outstanding = Requests
  /\ acked = {}
  /\ buffer = {}
  /\ wal = {}
  /\ objects = [i \in {} |-> {}]
  /\ manifest = {}
  /\ pending = {}
  /\ writing = None
  /\ compacting = <<>>
  /\ nextId = 1

Put(f, k, v) == [i \in DOMAIN f \cup {k} |-> IF i = k THEN v ELSE f[i]]
Drop(f, ks) == [i \in DOMAIN f \ ks |-> f[i]]

Receive(r) ==
  /\ r \in outstanding
  /\ buffer' = buffer \cup {r}
  /\ wal' = IF AckMode = "wal" THEN wal \cup {r} ELSE wal
  /\ UNCHANGED <<outstanding, acked, objects, manifest, pending, writing, compacting, nextId>>

AckReady(r) ==
  CASE AckMode = "hold" -> r \in Queryable
    [] AckMode = "wal" -> r \in wal \cup Queryable
    [] AckMode = "fast" -> r \in buffer \cup Queryable

Ack(r) ==
  /\ r \in outstanding
  /\ AckReady(r)
  /\ outstanding' = outstanding \ {r}
  /\ acked' = acked \cup {r}
  /\ UNCHANGED <<buffer, wal, objects, manifest, pending, writing, compacting, nextId>>

WriteSegment ==
  /\ writing = None
  /\ buffer # {}
  /\ nextId \in ObjectIds
  /\ objects' = Put(objects, nextId, buffer)
  /\ pending' = IF OrphanGuard THEN pending \cup {nextId} ELSE pending
  /\ writing' = nextId
  /\ nextId' = nextId + 1
  /\ UNCHANGED <<outstanding, acked, buffer, wal, manifest, compacting>>

CommitSegment ==
  /\ writing # None
  /\ manifest' = manifest \cup {writing}
  /\ LET rows == IF writing \in DOMAIN objects THEN objects[writing] ELSE {} IN
       /\ buffer' = buffer \ rows
       /\ wal' = wal \ rows
  /\ pending' = pending \ {writing}
  /\ writing' = None
  /\ UNCHANGED <<outstanding, acked, objects, compacting, nextId>>

WriteCompaction ==
  /\ compacting = <<>>
  /\ \E a, b \in manifest :
       /\ a < b
       /\ nextId \in ObjectIds
       /\ objects' = Put(objects, nextId, RowsOf({a, b}))
       /\ pending' = IF OrphanGuard THEN pending \cup {nextId} ELSE pending
       /\ compacting' = <<a, b, nextId>>
       /\ nextId' = nextId + 1
  /\ UNCHANGED <<outstanding, acked, buffer, wal, manifest, writing>>

CommitCompaction ==
  /\ compacting # <<>>
  /\ LET a == compacting[1] b == compacting[2] c == compacting[3] IN
       /\ manifest' = IF {a, b} \subseteq manifest THEN (manifest \ {a, b}) \cup {c} ELSE manifest
       /\ pending' = pending \ {c}
  /\ compacting' = <<>>
  /\ UNCHANGED <<outstanding, acked, buffer, wal, objects, writing, nextId>>

InFlight == {writing} \cup (IF compacting = <<>> THEN {} ELSE {compacting[3]})

SweepOrphan ==
  /\ \E o \in DOMAIN objects :
       /\ o \notin manifest
       /\ ~OrphanGuard \/ o \notin pending
       /\ objects' = Drop(objects, {o})
  /\ UNCHANGED <<outstanding, acked, buffer, wal, manifest, pending, writing, compacting, nextId>>

ReclaimPending ==
  /\ \E o \in pending :
       /\ o \notin manifest
       /\ o \notin InFlight
       /\ objects' = Drop(objects, {o})
       /\ pending' = pending \ {o}
  /\ UNCHANGED <<outstanding, acked, buffer, wal, manifest, writing, compacting, nextId>>

Evict ==
  /\ buffer' = IF AckMode = "wal" THEN wal ELSE {}
  /\ writing' = None
  /\ compacting' = <<>>
  /\ UNCHANGED <<outstanding, acked, wal, objects, manifest, pending, nextId>>

Next ==
  \/ \E r \in Requests : Receive(r) \/ Ack(r)
  \/ WriteSegment
  \/ CommitSegment
  \/ WriteCompaction
  \/ CommitCompaction
  \/ SweepOrphan
  \/ ReclaimPending
  \/ Evict

Spec == Init /\ [][Next]_vars

NoAckedLoss == acked \subseteq Durable

ManifestObjectsExist == manifest \subseteq DOMAIN objects

=============================================================================
