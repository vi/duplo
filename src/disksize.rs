use std::{sync::atomic::{AtomicU64, Ordering::SeqCst}, path::Path};

use tracing::error;

pub struct QuotaCounter {
    pub allowed: u64,
    pub current: AtomicU64,
}

impl QuotaCounter {
    pub fn get(&self) -> u64 {
        self.current.load(SeqCst)
    }
    /// Returns true if after adding the quota becomes exceeded (you need to manually `reduce` it then)
    pub fn bump(&self, val: u64) -> bool {
        self.current.fetch_add(val, SeqCst) + val > self.allowed
    }
    pub fn reduce(&self, val: u64) {
        if self.current.fetch_sub(val, SeqCst) < val {
            self.current.store(0, SeqCst);
        }
    }
    pub fn is_exceed(&self) -> bool {
        self.current.load(SeqCst) >= self.allowed
    }
    pub fn is_close_to_exeeed(&self) -> bool {
        self.current.load(SeqCst) as f32 >= self.allowed as f32 * 0.9
    }
    pub fn remaining(&self) -> u64 {
        self.allowed.saturating_sub(self.current.load(SeqCst))
    }
}

pub struct Quotas {
    pub bytes: QuotaCounter,
    pub files: QuotaCounter,
}

impl Quotas {
    pub fn new(files_limit: u64, bytes_limit: u64) -> Quotas {
        Quotas {
            bytes: QuotaCounter {
                allowed: bytes_limit,
                current: AtomicU64::new(0),
            },
            files: QuotaCounter {
                allowed: files_limit,
                current: AtomicU64::new(0),
            },
        }
    }

    pub fn scan_and_add(&self, dir: &Path) -> anyhow::Result<()> {
        let files = std::fs::read_dir(dir)?;
        let mut ctr1 = 0usize;
        let mut ctr2 = 0usize;
        for f in files {
            ctr1+=1;
            let Ok(f) = f else { continue }; 
            let Ok(meta) = f.metadata() else { continue }; 
            self.files.bump(1);
            self.bytes.bump(meta.len());
            // Note: not handling u64 overflows
            ctr2+=1;

        }
        if ctr1 != ctr2 {
            error!("Scanning some files for quota failed");
        }
        Ok(())
    }
}
