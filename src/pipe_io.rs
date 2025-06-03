use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;
use std::io::{BufReader, BufWriter, Read, Write};
use std::num::ParseIntError;
use thiserror::Error;

pub struct PipeReader {
    reader: BufReader<File>,
    buf: [u8; 1024],
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid format")]
    InvalidFormat,
    #[error("io error")]
    IOError(#[from] io::Error),
}

impl From<ParseIntError> for Error {
    fn from(_: ParseIntError) -> Self {
        Self::InvalidFormat
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Record {
    Version(u16),
    Exec(String),
    Image {
        name: String,
        start_address: usize,
        size: usize,
    },
    PageInfo {
        size: usize,
        pages: usize,
    },
    Trace {
        ip: usize,
        parent_idx: usize,
    },
    Alloc {
        ptr: usize,
        size: usize,
        parent_idx: usize,
    },
    Free {
        ptr: usize,
    },
    Duration(u128),
    RSS(usize),
}

impl PipeReader {
    pub fn new(file: File) -> Self {
        Self {
            reader: BufReader::with_capacity(4096, file),
            buf: [0; 1024],
        }
    }

    pub fn read_record(&mut self) -> Option<Result<Record, Error>> {
        let mut length_buf = [0u8; 2];
        if let Err(_) = self.reader.read_exact(&mut length_buf) {
            return None;
        }
        let len = u16::from_le_bytes(length_buf) as usize;

        let mut buf = &mut self.buf[..len];
        if let Err(e) = self.reader.read_exact(&mut buf) {
            return Some(Err(e.into()));
        }

        let record = bincode::deserialize(&buf).map_err(|_| Error::InvalidFormat);

        Some(record)
    }
}

pub struct PipeWriter {
    writer: BufWriter<File>,
}

impl PipeWriter {
    pub fn new(file: File) -> Self {
        Self {
            writer: BufWriter::with_capacity(4096, file),
        }
    }

    pub fn write_version(&mut self, version: u16) {
        let record = Record::Version(version);
        self.write_record(record)
    }

    pub fn write_image(&mut self, name: String, start_address: usize, size: usize) {
        let record = Record::Image {
            name,
            start_address,
            size,
        };
        self.write_record(record)
    }

    pub fn write_exec(&mut self, ex: &str) {
        let record = Record::Exec(ex.to_string());
        self.write_record(record)
    }

    pub fn write_page_info(&mut self, page_size: usize, phys_pages: usize) {
        let record = Record::PageInfo {
            size: page_size,
            pages: phys_pages,
        };
        self.write_record(record)
    }

    pub fn write_trace(&mut self, ip: usize, parent_idx: usize) {
        let record = Record::Trace { ip, parent_idx };
        self.write_record(record)
    }

    pub fn write_alloc(&mut self, size: usize, parent_idx: usize, ptr: usize) {
        let record = Record::Alloc {
            ptr,
            size,
            parent_idx,
        };
        self.write_record(record)
    }

    pub fn write_free(&mut self, ptr: usize) {
        let record = Record::Free { ptr };
        self.write_record(record)
    }

    pub fn write_duration(&mut self, duration: u128) {
        let record = Record::Duration(duration);
        self.write_record(record)
    }

    pub fn write_rss(&mut self, rss: usize) {
        let record = Record::RSS(rss);
        self.write_record(record)
    }

    fn write_record(&mut self, record: Record) {
        let s = bincode::serialize(&record).unwrap();
        _ = self.writer.write_all(&(s.len() as u16).to_le_bytes());
        _ = self.writer.write_all(&s);
    }

    pub fn flush(&mut self) {
        _ = self.writer.flush();
    }
}

#[cfg(test)]
mod tests {
    use crate::pipe_io::PipeReader;
    use std::fs::OpenOptions;

    #[test]
    fn test_read_record() {
        let file = OpenOptions::new().read(true).open("/tmp/trace").unwrap();
        let mut reader = PipeReader::new(file);

        let record = reader.read_record().unwrap();
        println!("{:?}", record);

        let record = reader.read_record().unwrap();
        println!("{:?}", record);

        let record = reader.read_record().unwrap();
        println!("{:?}", record);
    }
}
