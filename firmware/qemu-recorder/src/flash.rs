//! Simulator-only NOR model over a persistent semihosting host file.
//! Not an emulated physical flash controller; no fsync/electrical claims.
use embedded_storage_async::nor_flash::{
    ErrorType, MultiwriteNorFlash, NorFlash, NorFlashErrorKind, ReadNorFlash,
};
use semihosting::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
};

pub const CAPACITY: usize = 8192;
pub struct HostNor {
    pub mutations: u32,
    pub cut_after: Option<u32>,
    pub return_error: bool,
}
impl HostNor {
    fn mutate(&mut self, offset: u32, bytes: &[u8], erase: bool) -> Result<(), NorFlashErrorKind> {
        if offset as usize + bytes.len() > CAPACITY {
            return Err(NorFlashErrorKind::OutOfBounds);
        }
        if bytes.len() > 512 {
            return Err(NorFlashErrorKind::Other);
        }
        let amount = self
            .cut_after
            .map_or(bytes.len(), |n| (n as usize).min(bytes.len()));
        if amount > 0 {
            let mut file = File::options()
                .read(true)
                .write(true)
                .open(c"flash.bin")
                .map_err(|_| NorFlashErrorKind::Other)?;
            let mut old = [0u8; 512];
            file.seek(SeekFrom::Start(offset as u64))
                .map_err(|_| NorFlashErrorKind::Other)?;
            file.read_exact(&mut old[..amount])
                .map_err(|_| NorFlashErrorKind::Other)?;
            if !erase
                && old[..amount]
                    .iter()
                    .zip(&bytes[..amount])
                    .any(|(old, new)| old & new != *new)
            {
                return Err(NorFlashErrorKind::Other);
            }
            file.seek(SeekFrom::Start(offset as u64))
                .map_err(|_| NorFlashErrorKind::Other)?;
            file.write_all(&bytes[..amount])
                .map_err(|_| NorFlashErrorKind::Other)?;
            // Closing makes writes visible across QEMU process restarts. It is
            // not a guarantee against host power failure.
            drop(file);
            self.mutations += amount as u32;
        }
        if let Some(remaining) = &mut self.cut_after {
            *remaining -= amount as u32;
            if *remaining == 0 {
                if self.return_error {
                    self.cut_after = None;
                    return Err(NorFlashErrorKind::Other);
                }
                semihosting::process::exit(42);
            }
        }
        Ok(())
    }
}
impl ErrorType for HostNor {
    type Error = NorFlashErrorKind;
}
impl ReadNorFlash for HostNor {
    const READ_SIZE: usize = 1;
    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        if offset as usize + bytes.len() > CAPACITY {
            return Err(NorFlashErrorKind::OutOfBounds);
        }
        let mut file = File::open(c"flash.bin").map_err(|_| NorFlashErrorKind::Other)?;
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|_| NorFlashErrorKind::Other)?;
        file.read_exact(bytes).map_err(|_| NorFlashErrorKind::Other)
    }
    fn capacity(&self) -> usize {
        CAPACITY
    }
}
impl NorFlash for HostNor {
    const WRITE_SIZE: usize = 1;
    const ERASE_SIZE: usize = 4096;
    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        self.mutate(offset, bytes, false)
    }
    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        if !from.is_multiple_of(4096) || !to.is_multiple_of(4096) {
            return Err(NorFlashErrorKind::NotAligned);
        }
        if from > to || to as usize > CAPACITY {
            return Err(NorFlashErrorKind::OutOfBounds);
        }
        for offset in (from..to).step_by(256) {
            self.mutate(offset, &[0xff; 256], true)?;
        }
        Ok(())
    }
}
impl MultiwriteNorFlash for HostNor {}
