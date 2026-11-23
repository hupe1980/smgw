//! One atomic, durable maximum signed timestamp per gateway recipient.

use redb::{
    ConcurrencyMode, Database, Durability, ReadableDatabase, ReadableTable, TableDefinition,
    TableError, TransactionError, WriteTransaction,
};
use std::{
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

const WATERMARKS: TableDefinition<&[u8; 9], i64> = TableDefinition::new("wakeup_replay");
const WRITER_BUDGET: Duration = Duration::from_millis(500);

pub(super) struct ReplayStore(Database);

impl ReplayStore {
    pub(super) fn open(path: &Path) -> Result<Self, redb::Error> {
        if path.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a replay database path",
            )
            .into());
        }
        match fs::metadata(path) {
            Ok(info) if !info.is_file() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "expected a regular replay database file",
                )
                .into());
            }
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        let database = Database::builder()
            .set_cache_size(1024 * 1024)
            .set_concurrency_mode(ConcurrencyMode::MultiWriter)
            .set_check_integrity_on_open(true)
            .create(path)?;
        Ok(Self(database))
    }

    pub(super) fn watermark(&self, recipient: &[u8; 9]) -> Result<Option<i64>, redb::Error> {
        let transaction = self.0.begin_read()?;
        match transaction.open_table(WATERMARKS) {
            Ok(table) => Ok(table.get(recipient)?.map(|entry| entry.value())),
            Err(TableError::TableDoesNotExist(_)) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn begin_claim(&self) -> Result<WriteTransaction, redb::Error> {
        let start = Instant::now();
        loop {
            match self.0.try_begin_write() {
                Err(TransactionError::WouldBlock) => {
                    let remaining = WRITER_BUDGET.saturating_sub(start.elapsed());
                    if remaining.is_zero() {
                        return Err(redb::Error::WouldBlock);
                    }
                    std::thread::sleep(remaining.min(Duration::from_millis(5)));
                }
                result => return Ok(result?),
            }
        }
    }

    /// Compare the current watermark and update it while holding the same writer
    /// lock. Successful verification is reported only after the durable commit.
    pub(super) fn advance(&self, recipient: &[u8; 9], timestamp: i64) -> Result<bool, redb::Error> {
        let mut transaction = self.begin_claim()?;
        transaction.set_durability(Durability::Immediate)?;
        {
            let mut table = transaction.open_table(WATERMARKS)?;
            if table
                .get(recipient)?
                .is_some_and(|current| current.value() >= timestamp)
            {
                return Ok(false);
            }
            table.insert(recipient, timestamp)?;
        }
        transaction.commit()?;
        Ok(true)
    }
}
