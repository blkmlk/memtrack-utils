use crate::output::{Frame, Output};
use crate::pipe_io::Record;
use crate::resolver::Resolver;
use crate::{executor, resolver};
use indexmap::{IndexMap, IndexSet};
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Execution failed")]
    Exec(#[from] executor::Error),
    #[error("IO error")]
    Io(#[from] io::Error),
    #[error("Resolver")]
    Resolver(#[from] resolver::Error),
    #[error("Custom error: {0}")]
    Custom(String),
}

#[derive(Default)]
struct MemStats {
    allocations: u64,
    leaked_allocations: u64,
    tmp_allocations: u64,
}

#[derive(Hash, PartialEq, Eq)]
struct AllocationInfo {
    size: u64,
    trace_idx: u64,
}

const PAGE_SIZE: u64 = u16::MAX as u64 / 4;

struct SplitPointer {
    big: u64,
    small: u16,
}

impl SplitPointer {
    pub fn new(ptr: u64) -> Self {
        Self {
            big: ptr / PAGE_SIZE,
            small: (ptr % PAGE_SIZE) as u16,
        }
    }
}

#[derive(Default)]
struct Indices {
    small_ptr_parts: Vec<u16>,
    allocation_indices: Vec<usize>,
}

pub struct Interpreter {
    output: Output,
    strings: IndexSet<String>,
    frames: IndexSet<u64>,
    pointers: IndexMap<u64, Indices>,
    allocation_info: IndexSet<AllocationInfo>,
    resolver: Resolver,
    stats: MemStats,
    last_ptr: usize,
}

impl Interpreter {
    pub fn new(out_filepath: impl AsRef<Path>) -> io::Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(out_filepath)?;

        Ok(Self {
            output: Output::new(file),
            strings: IndexSet::new(),
            frames: IndexSet::new(),
            pointers: IndexMap::new(),
            allocation_info: IndexSet::new(),
            resolver: Resolver::new(),
            stats: MemStats::default(),
            last_ptr: 0,
        })
    }

    pub fn exec<S, P>(
        &mut self,
        program: S,
        args: impl IntoIterator<Item = S>,
        cwd: P,
        lib_path: &str,
    ) -> Result<(), Error>
    where
        S: AsRef<OsStr>,
        P: AsRef<Path>,
    {
        let mut exec = executor::exec_cmd(program, args, cwd, lib_path);

        while let Some(item) = exec.next() {
            let record = item?;

            self.handle_record(record)?;
        }

        self.write_comments()?;

        self.output.flush()?;

        Ok(())
    }

    fn handle_record(&mut self, record: Record) -> Result<(), Error> {
        match record {
            Record::Version(version) => {
                self.output.write_version(version, 3)?;
            }
            Record::Exec(cmd) => {
                self.output.write_exec(&cmd)?;
            }
            Record::Image {
                name,
                start_address,
                size,
            } => {
                let module_id = self.write_string(&name)?;
                _ = self.resolver.add_module(
                    module_id,
                    &name,
                    start_address as u64,
                    start_address as u64 + size as u64,
                );
            }
            Record::PageInfo { size, pages } => {
                self.output.write_page_info(size, pages as u64)?;
            }
            Record::Trace { ip, parent_idx } => {
                let ip_id = self.add_frame(ip as u64)?;
                self.output.write_trace(ip_id, parent_idx as u64)?;
            }
            Record::Alloc {
                ptr,
                size,
                parent_idx,
            } => {
                self.stats.allocations += 1;
                self.stats.leaked_allocations += 1;

                let idx = self.add_alloc(size as u64, parent_idx as u64)?;

                self.add_pointer(ptr as u64, idx as u64);
                self.last_ptr = ptr;
                self.output.write_alloc(idx)?;
            }
            Record::Free { ptr } => {
                let temporary = self.last_ptr == ptr;
                self.last_ptr = 0;

                let Some(allocation_idx) = self.take_pointer(ptr as u64) else {
                    return Ok(());
                };

                self.output.write_free(allocation_idx)?;

                if temporary {
                    self.stats.tmp_allocations += 1;
                }
                self.stats.leaked_allocations -= 1;
            }
            Record::Duration(duration) => {
                self.output.write_duration(duration)?;
            }
            Record::RSS(rss) => {
                self.output.write_rss(rss)?;
            }
        }

        Ok(())
    }

    fn add_frame(&mut self, ip: u64) -> Result<usize, Error> {
        match self.frames.get_full(&ip) {
            None => {
                let (id, _) = self.frames.insert_full(ip);

                let Some(result) = self.resolver.lookup(ip) else {
                    return Err(Error::Custom("ip locations not found".to_string()));
                };

                let mut frames = Vec::with_capacity(result.locations.len());

                for location in result.locations {
                    let function_idx = self.write_string(&location.function_name)?;

                    let frame = if location.file_name.is_some() {
                        let file_idx = self.write_string(
                            &location
                                .file_name
                                .ok_or_else(|| Error::Custom("empty file name".into()))?,
                        )?;

                        let line_number = location
                            .line_number
                            .ok_or_else(|| Error::Custom("empty line number".into()))?;

                        Frame::Multiple {
                            function_idx,
                            file_idx,
                            line_number,
                        }
                    } else {
                        Frame::Single { function_idx }
                    };

                    frames.push(frame);
                }

                self.output
                    .write_instruction(ip, result.module_id, &frames)?;

                Ok(id + 1)
            }
            Some((id, _)) => Ok(id + 1),
        }
    }

    fn add_alloc(&mut self, size: u64, parent_idx: u64) -> Result<usize, Error> {
        let info = AllocationInfo {
            size,
            trace_idx: parent_idx,
        };

        match self.allocation_info.get_full(&info) {
            None => {
                let (idx, _) = self.allocation_info.insert_full(info);

                self.output.write_trace_alloc(size, parent_idx as usize)?;

                Ok(idx)
            }
            Some((idx, _)) => Ok(idx),
        }
    }

    fn add_pointer(&mut self, ptr: u64, allocation_idx: u64) {
        let pointer = SplitPointer::new(ptr);

        let indices = self.pointers.entry(pointer.big).or_default();

        match indices
            .small_ptr_parts
            .iter()
            .position(|&i| i == pointer.small)
        {
            None => {
                indices.small_ptr_parts.push(pointer.small);
                indices.allocation_indices.push(allocation_idx as usize);
            }
            Some(idx) => {
                indices.allocation_indices[idx] = allocation_idx as usize;
            }
        }
    }

    fn take_pointer(&mut self, ptr: u64) -> Option<usize> {
        let pointer = SplitPointer::new(ptr);
        let indices = self.pointers.get_mut(&pointer.big)?;

        let idx = indices
            .small_ptr_parts
            .iter()
            .position(|&i| i == pointer.small)?;
        let allocation_idx = indices.allocation_indices[idx];

        indices.small_ptr_parts.swap_remove(idx);
        indices.allocation_indices.swap_remove(idx);
        if indices.allocation_indices.is_empty() {
            self.pointers.swap_remove(&pointer.big);
        }

        Some(allocation_idx)
    }

    fn write_string(&mut self, value: &str) -> Result<usize, Error> {
        match self.strings.get_full(value) {
            None => {
                let (id, _) = self.strings.insert_full(value.to_string());
                self.output.write_string(value)?;

                Ok(id + 1)
            }
            Some((id, _)) => Ok(id + 1),
        }
    }

    fn write_comments(&mut self) -> Result<(), Error> {
        self.output.write("")?;

        self.output
            .write_comment(&format!("strings: {}", self.strings.len()))?;
        self.output
            .write_comment(&format!("ips: {}", self.frames.len()))?;

        Ok(())
    }
}
