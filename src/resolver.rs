use addr2line::Loader;
use rangemap::RangeMap;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("module not found")]
    ModuleNotFound,
}

#[derive(Clone, Eq, PartialEq)]
struct Module {
    id: usize,
    pub start_address: u64,
    pub end_address: u64,
    path: String,
}

impl Module {
    pub fn new(id: usize, path: String, start_address: u64, size: u64) -> Self {
        Self {
            id,
            path,
            start_address,
            end_address: start_address + size,
        }
    }

    pub fn lookup(&self, ip: u64, loader: &Loader) -> Option<LookupResult> {
        let mut locations = Vec::new();

        let mut iter = loader.find_frames(ip).unwrap();
        while let Some(frame) = iter.next().unwrap() {
            let function_name =
                rustc_demangle::demangle(frame.function.unwrap().name.to_string().unwrap())
                    .to_string();
            let location = match frame.location {
                Some(location) => Location {
                    function_name,
                    file_name: Some(location.file.unwrap().to_string()),
                    line_number: Some(location.line.unwrap_or_default()),
                },
                None => Location {
                    function_name,
                    file_name: None,
                    line_number: None,
                },
            };

            locations.push(location);
        }

        if locations.is_empty() {
            let symbol = loader.find_symbol(ip).unwrap();
            let function_name = rustc_demangle::demangle(symbol).to_string();

            locations.push(Location {
                function_name,
                file_name: None,
                line_number: None,
            })
        }

        Some(LookupResult {
            module_id: self.id,
            locations,
        })
    }
}

#[derive(Clone, Debug)]
pub struct LookupResult {
    pub module_id: usize,
    pub locations: Vec<Location>,
}

#[derive(Clone, Debug)]
pub struct Location {
    pub function_name: String,
    pub file_name: Option<String>,
    pub line_number: Option<u32>,
}

pub struct Resolver {
    modules: RangeMap<u64, Module>,
    cached: HashMap<u64, LookupResult>,
    loaders: HashMap<u64, Loader>,
}

impl Resolver {
    pub fn new() -> Self {
        Self {
            modules: RangeMap::new(),
            cached: HashMap::new(),
            loaders: HashMap::new(),
        }
    }

    pub fn add_module(
        &mut self,
        id: usize,
        file_path: &str,
        start_address: u64,
        size: u64,
    ) -> Result<(), Error> {
        let module = Module::new(id, file_path.to_string(), start_address, size);

        let Ok(loader) = Loader::new(file_path.to_string()) else {
            return Err(Error::ModuleNotFound);
        };

        self.loaders.insert(start_address, loader);

        self.modules
            .insert(module.start_address..module.end_address, module);

        Ok(())
    }

    pub fn lookup(&mut self, ip: u64) -> Option<LookupResult> {
        if let Some(location) = self.cached.get(&ip).cloned() {
            return Some(location);
        }

        let module = self.modules.get(&ip)?;
        let loader = self.loaders.get(&module.start_address)?;

        let locations = module.lookup(ip, loader)?;

        self.cached.insert(ip, locations.clone());

        Some(locations)
    }
}

#[cfg(test)]
mod tests {
    use crate::resolver::Resolver;
    use std::ffi::c_void;

    extern "C" {
        fn _dyld_get_image_header(index: u32) -> *const c_void;
        fn _dyld_get_image_vmaddr_slide(index: u32) -> isize;
    }

    fn boo() {}

    #[test]
    fn test_lookup() {
        let exe = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let slide = unsafe { _dyld_get_image_vmaddr_slide(0) } as u64;

        let addr = unsafe { _dyld_get_image_header(0) } as u64 - slide;

        let mut resolver = Resolver::new();
        resolver.add_module(0, &exe, addr, 0x1000000);

        let ip = boo as u64 - slide;

        let res = resolver.lookup(ip);
        println!("{:#?}", res);
    }

    #[test]
    fn test_lookup_binary() {
        let exe = "/Users/id/devel/Rust/memtrack-rs/.local/simple";
        let addr = 0x100001874;

        let mut resolver = Resolver::new();
        resolver.add_module(0, exe, 0x100000000, 0x1000000).unwrap();

        let res = resolver.lookup(addr);
        println!("{:#?}", res);
    }
}
