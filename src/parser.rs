use indexmap::map::Entry;
use indexmap::IndexMap;
use std::fs::OpenOptions;
use std::io;
use std::io::BufRead;
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("Invalid format")]
    InvalidFormat,
    #[error("Internal {0}")]
    Internal(String),
}

#[derive(Debug)]
pub struct Trace {
    pub ip_idx: u64,
    pub parent_idx: u64,
}

#[derive(Debug)]
pub struct InstructionPointer {
    pub ip: u64,
    pub module_idx: usize,
    pub frame: Frame,
    pub inlined: Vec<Frame>,
}

#[derive(Debug)]
pub enum Frame {
    Single {
        function_idx: usize,
    },
    Multiple {
        function_idx: usize,
        file_idx: usize,
        line_number: u32,
    },
}

#[derive(Debug, Default)]
pub struct AllocationData {
    pub allocations: u64,
    pub temporary: u64,
    pub leaked: u64,
    pub peak: u64,
}

#[derive(Debug)]
pub struct AllocationInfo {
    pub allocation_idx: u64,
    pub size: u64,
}

impl AllocationInfo {
    pub fn new(allocation_idx: u64, size: u64) -> Self {
        Self {
            allocation_idx,
            size,
        }
    }
}

#[derive(Debug)]
pub struct Allocation {
    pub trace_idx: u64,
    pub data: AllocationData,
}

impl Allocation {
    pub fn new(trace_idx: u64) -> Self {
        Self {
            trace_idx,
            data: Default::default(),
        }
    }
}

#[derive(Debug)]
pub struct AccumulatedData {
    pub strings: Vec<String>,
    pub traces: Vec<Trace>,
    pub instruction_pointers: Vec<InstructionPointer>,
    pub allocation_indices: IndexMap<u64, u64>,
    pub allocation_infos: Vec<AllocationInfo>,
    pub allocations: Vec<Allocation>,
    pub total: AllocationData,
    pub duration: Duration,
    pub peak_rss: u64,
    pub page_size: u64,
    pub pages: u64,
}

impl AccumulatedData {
    pub fn new() -> Self {
        Self {
            strings: Vec::with_capacity(4096),
            traces: Vec::with_capacity(65536),
            instruction_pointers: Vec::with_capacity(16384),
            allocation_indices: IndexMap::with_capacity(16384),
            allocations: Vec::with_capacity(16384),
            allocation_infos: Vec::with_capacity(16384),
            total: AllocationData::default(),
            duration: Duration::default(),
            peak_rss: 0,
            page_size: 0,
            pages: 0,
        }
    }
}

pub struct Parser {
    data: AccumulatedData,
    last_ptr: u64,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            data: AccumulatedData::new(),
            last_ptr: 0,
        }
    }

    pub fn parse_file(mut self, file_path: impl AsRef<Path>) -> Result<AccumulatedData, Error> {
        let file = OpenOptions::new().read(true).open(file_path)?;
        let reader = io::BufReader::new(file);

        for line in reader.lines() {
            self.parse_line(&line?)?
        }

        Ok(self.data)
    }

    fn parse_line(&mut self, line: &str) -> Result<(), Error> {
        let mut split = line.split_whitespace();

        let Some(first) = split.next() else {
            return Ok(());
        };

        match first {
            "s" => {
                let str_len = usize::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                    .map_err(|_| Error::InvalidFormat)?;
                self.data
                    .strings
                    .push(line[line.len() - str_len..].to_string());
            }
            "t" => {
                let ip_idx = u64::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                    .map_err(|_| Error::InvalidFormat)?;
                let parent_idx = u64::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                    .map_err(|_| Error::InvalidFormat)?;

                self.data.traces.push(Trace { ip_idx, parent_idx })
            }
            "i" => {
                let ip = u64::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                    .map_err(|_| Error::InvalidFormat)?;
                let module_idx =
                    usize::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                        .map_err(|_| Error::InvalidFormat)?;

                let frame = Self::parse_frame(&mut split)?.ok_or(Error::InvalidFormat)?;
                let mut inlined = Vec::new();

                while let Some(frame) = Self::parse_frame(&mut split)? {
                    inlined.push(frame);
                }

                self.data.instruction_pointers.push(InstructionPointer {
                    ip,
                    module_idx,
                    frame,
                    inlined,
                })
            }
            "a" => {
                let size = u64::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                    .map_err(|_| Error::InvalidFormat)?;
                let trace_idx = u64::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                    .map_err(|_| Error::InvalidFormat)?;

                let allocation_idx = self.add_allocation(trace_idx);
                self.data
                    .allocation_infos
                    .push(AllocationInfo::new(allocation_idx, size));
            }
            "+" => {
                let allocation_info_idx =
                    u64::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                        .map_err(|_| Error::InvalidFormat)?;

                let info = &mut self.data.allocation_infos[allocation_info_idx as usize];

                let allocation = self
                    .data
                    .allocations
                    .get_mut(info.allocation_idx as usize)
                    .ok_or_else(|| Error::Internal("allocation not found".into()))?;

                self.last_ptr = info.allocation_idx;

                allocation.data.leaked += info.size;
                if allocation.data.leaked > allocation.data.peak {
                    allocation.data.peak = allocation.data.leaked;
                }
                allocation.data.allocations += 1;

                self.data.total.leaked += info.size;
                self.data.total.allocations += 1;

                if self.data.total.leaked > self.data.total.peak {
                    self.data.total.peak = self.data.total.leaked;
                }
            }
            "-" => {
                let allocation_info_idx =
                    u64::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                        .map_err(|_| Error::InvalidFormat)?;

                let info = &mut self.data.allocation_infos[allocation_info_idx as usize];

                let allocation = self
                    .data
                    .allocations
                    .get_mut(info.allocation_idx as usize)
                    .ok_or_else(|| Error::Internal("allocation not found".into()))?;

                self.data.total.leaked -= info.size;

                let temporary = self.last_ptr == info.allocation_idx;
                self.last_ptr = 0;

                if temporary {
                    self.data.total.temporary += 1;
                }

                allocation.data.leaked -= info.size;
                if temporary {
                    allocation.data.temporary += 1;
                }
            }
            "c" => {
                let timestamp = u64::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                    .map_err(|_| Error::InvalidFormat)?;
                self.data.duration = Duration::from_millis(timestamp);
            }
            "R" => {
                let rss = u64::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                    .map_err(|_| Error::InvalidFormat)?;
                if rss > self.data.peak_rss {
                    self.data.peak_rss = rss;
                }
            }
            "I" => {
                self.data.page_size =
                    u64::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                        .map_err(|_| Error::InvalidFormat)?;
                self.data.pages =
                    u64::from_str_radix(split.next().ok_or(Error::InvalidFormat)?, 16)
                        .map_err(|_| Error::InvalidFormat)?;
            }
            "#" => {
                // comment
            }
            _ => {}
        }
        Ok(())
    }

    fn add_allocation(&mut self, trace_idx: u64) -> u64 {
        match self.data.allocation_indices.entry(trace_idx) {
            Entry::Occupied(e) => *e.get(),
            Entry::Vacant(e) => {
                let idx = self.data.allocations.len() as u64;
                e.insert(idx);
                let allocation = Allocation::new(trace_idx);
                self.data.allocations.push(allocation);
                idx
            }
        }
    }

    fn parse_frame<'a>(mut iter: impl Iterator<Item = &'a str>) -> Result<Option<Frame>, Error> {
        let Some(first) = iter.next() else {
            return Ok(None);
        };

        let function_idx = usize::from_str_radix(first, 16).map_err(|_| Error::InvalidFormat)?;

        let Some(file_val) = iter.next() else {
            return Ok(Some(Frame::Single { function_idx }));
        };

        let file_idx = usize::from_str_radix(file_val, 16).map_err(|_| Error::InvalidFormat)?;
        let line_number = u32::from_str_radix(iter.next().ok_or(Error::InvalidFormat)?, 16)
            .map_err(|_| Error::InvalidFormat)?;

        Ok(Some(Frame::Multiple {
            function_idx,
            file_idx,
            line_number,
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::Parser;

    #[test]
    fn test_read_trace_file() {
        let file = "/tmp/pipe.out";
        let data = Parser::new().parse_file(file).unwrap();

        println!("{:#?}", data);
    }
}
