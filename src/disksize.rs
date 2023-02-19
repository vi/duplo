use std::{sync::{atomic::{AtomicU64, Ordering::SeqCst}, Arc}, path::Path, time::{Duration, SystemTime}};

use tracing::{error, debug, info};

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

pub fn cleanup_task(transient_dir: &Path, cleanup_time: time::Time, max_age: Duration, quotas: Arc<Quotas>) -> anyhow::Result<()> { 
    loop {
        let begin = time::OffsetDateTime::now_utc();
        let mut next_cleanup = begin.replace_time(cleanup_time);
        if next_cleanup < begin {
            next_cleanup += Duration::from_secs(24*3600);
        }
        let to_wait : Duration = (next_cleanup - begin).try_into()?;
        debug!("Cleanup task is waiting for {:?}", to_wait);
        std::thread::sleep(to_wait);

        let mut bytes_retained = 0u64;
        let mut files_retained = 0u64;
        let mut bytes_removed = 0u64;
        let mut files_removed = 0u64;
        let mut errors = 0u64;

        let now = SystemTime::now();

        let files = std::fs::read_dir(transient_dir)?;
        for f in files {
            errors+=1;
            let Ok(f) = f else { continue };
            let Ok(meta) = f.metadata() else { continue };
            if meta.is_dir() || meta.is_symlink() { errors-=1; continue; }
            let Ok(modified) = meta.modified() else { continue };

            let retain = match now.duration_since(modified) {
                Ok(x) => {
                    debug!("File {:?} has age {x:?}", f.path());
                    x < max_age
                },
                Err(_) => true,
            };

            if retain {
                files_retained+=1;
                bytes_retained+=meta.len();
                errors-=1;
            } else {
                match std::fs::remove_file(f.path()) {
                    Ok(()) => {
                        files_removed+=1;
                        bytes_removed+=meta.len();
                        quotas.files.reduce(1);
                        quotas.bytes.reduce(meta.len());
                        errors-=1;
                    }
                    Err(e) => {
                        info!("Error removing file {:?}: {e}", f.path())
                    }
                }
            }
        }

        println!("cleanup, removed {files_removed} files ({bytes_removed} bytes), retained {files_retained} files ({bytes_retained} bytes); {errors} errors");

        std::thread::sleep(Duration::from_secs(60));
    }
}
