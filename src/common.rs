use std::{fs, io};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use anyhow::Context;

pub fn download_lib_if_needed(lib_dir: impl AsRef<Path>, lib_version: &str) -> anyhow::Result<String> {
    if lib_dir.as_ref().is_file() {
        anyhow::bail!("lib_dir is not a directory");
    }

    let lib_file = lib_dir
        .as_ref()
        .join(format!("libmemtrack_{}.dylib", lib_version));

    if lib_file.exists() {
        return Ok(lib_file.to_str().unwrap().to_string());
    }

    println!("Loading libmemtrack version {}", lib_version);

    fs::create_dir_all(lib_dir).context("failed to create dirs")?;

    let mut response = reqwest::blocking::get(format!(
        "https://github.com/blkmlk/memtrack-lib/releases/download/{}/libmemtrack_lib.dylib",
        lib_version
    ))
        .context("failed to download libmemtrack.dylib")?;

    if !response.status().is_success() {
        anyhow::bail!(
            "failed to download libmemtrack.dylib. status: {}",
            response.status()
        );
    }

    let mut out_file =
        BufWriter::new(File::create(&lib_file).context("failed to create output file")?);

    io::copy(&mut response, &mut out_file).context("failed to write output file")?;

    println!(
        "Successfully loaded libmemtrack.dylib version {}",
        lib_version
    );

    Ok(lib_file.to_str().unwrap().to_string())
}