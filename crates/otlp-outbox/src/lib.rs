#![no_std]
//! Experimental outbox policy, not an OTLP semantic validator or network client.
//! Original protobuf bytes are never decoded/re-encoded. Caller supplies unique,
//! persistent batch identities and authenticates application ACKs externally.

use core::ops::Range;
use embedded_storage_async::nor_flash::MultiwriteNorFlash;
use sequential_storage::{
    cache::{Cache, Uncached},
    queue::{QueueConfig, QueueStorage},
};

pub const HEADER_SIZE: usize = 28;
pub const ACK_SIZE: usize = 24;
type NoCache = Cache<Uncached, Uncached, Uncached>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Signal {
    Logs = 1,
    Traces = 2,
    Metrics = 3,
}
impl TryFrom<u8> for Signal {
    type Error = WireError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Logs),
            2 => Ok(Self::Traces),
            3 => Ok(Self::Metrics),
            _ => Err(WireError),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchId(pub [u8; 16]);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireError;
#[derive(Debug)]
pub enum Error<E> {
    Storage(sequential_storage::Error<E>),
    Wire,
    BufferTooSmall,
    Empty,
    MismatchedAck,
}

#[derive(Debug)]
pub struct Batch<'a> {
    pub id: BatchId,
    pub signal: Signal,
    pub payload: &'a [u8],
    wire: &'a [u8],
}
impl<'a> Batch<'a> {
    pub fn parse(wire: &'a [u8]) -> Result<Self, WireError> {
        if wire.len() < HEADER_SIZE || &wire[..4] != b"SOB1" || wire[5..8] != [0; 3] {
            return Err(WireError);
        }
        let len = u32::from_be_bytes(wire[24..28].try_into().map_err(|_| WireError)?) as usize;
        if len != wire.len() - HEADER_SIZE {
            return Err(WireError);
        }
        Ok(Self {
            id: BatchId(wire[8..24].try_into().map_err(|_| WireError)?),
            signal: wire[4].try_into()?,
            payload: &wire[28..],
            wire,
        })
    }
    pub fn wire(&self) -> &'a [u8] {
        self.wire
    }
}

/// Only FullAcceptance can remove data. Other outcomes require caller policy.
/// Partial/permanent outcomes must stop automatic retries until a durable
/// quarantine policy exists; this prototype does not implement that policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Outcome {
    FullAcceptance = 1,
    BrokerOnly = 2,
    RetryLater = 3,
    Partial = 4,
    Permanent = 5,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ack {
    pub id: BatchId,
    pub signal: Signal,
    pub outcome: Outcome,
}
impl Ack {
    /// Experimental simulator/gateway profile; not a standardized OTLP message.
    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() != ACK_SIZE || &bytes[..4] != b"SAK1" || bytes[22..24] != [0; 2] {
            return Err(WireError);
        }
        let outcome = match bytes[21] {
            1 => Outcome::FullAcceptance,
            2 => Outcome::BrokerOnly,
            3 => Outcome::RetryLater,
            4 => Outcome::Partial,
            5 => Outcome::Permanent,
            _ => return Err(WireError),
        };
        Ok(Self {
            id: BatchId(bytes[4..20].try_into().map_err(|_| WireError)?),
            signal: bytes[20].try_into()?,
            outcome,
        })
    }
    pub fn encode(self) -> [u8; ACK_SIZE] {
        let mut bytes = [0; ACK_SIZE];
        bytes[..4].copy_from_slice(b"SAK1");
        bytes[4..20].copy_from_slice(&self.id.0);
        bytes[20] = self.signal as u8;
        bytes[21] = self.outcome as u8;
        bytes
    }
}

pub struct Outbox<S: MultiwriteNorFlash> {
    queue: QueueStorage<S, NoCache>,
}
impl<S: MultiwriteNorFlash> Outbox<S> {
    /// Range must meet sequential-storage's configuration requirements.
    pub fn new(storage: S, range: Range<u32>) -> Self {
        Self {
            queue: QueueStorage::new(storage, QueueConfig::new(range), Cache::new_uncached()),
        }
    }
    pub async fn push(
        &mut self,
        id: BatchId,
        signal: Signal,
        payload: &[u8],
        scratch: &mut [u8],
    ) -> Result<(), Error<S::Error>> {
        let len = HEADER_SIZE
            .checked_add(payload.len())
            .ok_or(Error::BufferTooSmall)?;
        if scratch.len() < len || payload.len() > u32::MAX as usize {
            return Err(Error::BufferTooSmall);
        }
        let bytes = &mut scratch[..len];
        bytes[..4].copy_from_slice(b"SOB1");
        bytes[4] = signal as u8;
        bytes[5..8].fill(0);
        bytes[8..24].copy_from_slice(&id.0);
        bytes[24..28].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes[28..].copy_from_slice(payload);
        // Never overwrite pending telemetry when storage is full.
        self.queue.push(bytes, false).await.map_err(Error::Storage)
    }
    pub async fn peek<'a>(
        &mut self,
        scratch: &'a mut [u8],
    ) -> Result<Option<Batch<'a>>, Error<S::Error>> {
        self.queue
            .peek(scratch)
            .await
            .map_err(Error::Storage)?
            .map(|bytes| Batch::parse(bytes).map_err(|_| Error::Wire))
            .transpose()
    }
    /// Caller must authenticate the ACK, validate the downstream response and
    /// choose a documented acceptance boundary before constructing it.
    pub async fn apply_ack(
        &mut self,
        ack: Ack,
        scratch: &mut [u8],
    ) -> Result<bool, Error<S::Error>> {
        let head = self.peek(scratch).await?.ok_or(Error::Empty)?;
        if head.id != ack.id || head.signal != ack.signal {
            return Err(Error::MismatchedAck);
        }
        if ack.outcome != Outcome::FullAcceptance {
            return Ok(false);
        }
        self.queue
            .pop(scratch)
            .await
            .map_err(Error::Storage)?
            .ok_or(Error::Empty)?;
        Ok(true)
    }
    pub fn destroy(self) -> S {
        self.queue.destroy().0
    }
}
