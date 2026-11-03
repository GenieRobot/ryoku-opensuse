//! Index checksum-pinned RPM-MD primary metadata after detached-signature review.
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha512};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::Path,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn attributes(element: &BytesStart<'_>) -> Result<Map<String, Value>> {
    let mut result = Map::new();
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = std::str::from_utf8(attribute.key.as_ref())?.to_owned();
        let value = attribute.normalized_value(quick_xml::XmlVersion::Implicit1_0)?.into_owned();
        if result.insert(key, Value::String(value)).is_some() {
            return Err("duplicate metadata attribute".into());
        }
    }
    Ok(result)
}

fn index<R: BufRead, W: Write>(input: R, destination: &mut W) -> Result<u64> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut declared = None;
    let mut ended = false;
    let mut package: Option<Map<String, Value>> = None;
    let mut current = Vec::new();
    let mut dependency = None;
    let mut count = 0;
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => match element.name().as_ref() {
                b"metadata" if declared.is_none() => {
                    declared = Some(
                        attributes(&element)?
                            .get("packages")
                            .and_then(Value::as_str)
                            .ok_or("missing package count")?
                            .parse::<u64>()?,
                    );
                }
                b"metadata" => return Err("duplicate metadata root".into()),
                b"package" if package.is_none() => {
                    package = Some(Map::new());
                }
                b"name" | b"arch" | b"checksum" if package.is_some() => {
                    current = element.name().as_ref().to_vec();
                    if current == b"checksum" {
                        let algorithm = attributes(&element)?
                            .get("type")
                            .and_then(Value::as_str)
                            .ok_or("missing package checksum type")?
                            .to_owned();
                        if !matches!(algorithm.as_str(), "sha256" | "sha512") {
                            return Err("unsupported package checksum".into());
                        }
                        package
                            .as_mut()
                            .ok_or("missing package")?
                            .insert("checksum_type".into(), Value::String(algorithm));
                    }
                }
                b"rpm:provides" => {
                    dependency = Some("provides");
                }
                b"rpm:requires" => {
                    dependency = Some("requires");
                }
                _ => {}
            },
            Event::Empty(element) => {
                if let Some(package) = &mut package {
                    match element.name().as_ref() {
                        b"version" | b"location" => {
                            let key = std::str::from_utf8(element.name().as_ref())?.to_owned();
                            if package.insert(key, Value::Object(attributes(&element)?)).is_some() {
                                return Err("duplicate package identity".into());
                            }
                        }
                        b"rpm:entry" => {
                            if let Some(kind) = dependency {
                                package
                                    .entry(kind)
                                    .or_insert_with(|| Value::Array(Vec::new()))
                                    .as_array_mut()
                                    .ok_or("invalid dependency list")?
                                    .push(Value::Object(attributes(&element)?));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::Text(value) if !current.is_empty() => {
                let decoded = value.decode()?;
                let value = quick_xml::escape::unescape(&decoded)?.into_owned();
                let key = std::str::from_utf8(&current)?.to_owned();
                if package
                    .as_mut()
                    .ok_or("text outside package")?
                    .insert(key, Value::String(value))
                    .is_some()
                {
                    return Err("duplicate or split package identity".into());
                }
            }
            Event::End(element) => match element.name().as_ref() {
                b"metadata" => {
                    ended = true;
                }
                b"name" | b"arch" | b"checksum" => {
                    current.clear();
                }
                b"rpm:provides" | b"rpm:requires" => {
                    dependency = None;
                }
                b"package" => {
                    let package = package.take().ok_or("unmatched package")?;
                    for key in ["name", "arch", "checksum", "checksum_type", "version", "location"]
                    {
                        if !package.contains_key(key) {
                            return Err(format!("package missing {key}").into());
                        }
                    }
                    let digest = package["checksum"].as_str().ok_or("invalid package checksum")?;
                    let length = if package["checksum_type"] == "sha256" { 64 } else { 128 };
                    if digest.len() != length
                        || !digest
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    {
                        return Err("invalid package checksum digest".into());
                    }
                    serde_json::to_writer(&mut *destination, &package)?;
                    destination.write_all(b"\n")?;
                    count += 1;
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if package.is_some() || !ended || declared != Some(count) {
        return Err("truncated metadata/package count mismatch".into());
    }
    Ok(count)
}

struct Hashed<R> {
    inner: R,
    hash: Sha512,
    bytes: u64,
}
impl<R: Read> Read for Hashed<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(bytes)?;
        self.bytes += count as u64;
        if self.bytes > 1024 * 1024 * 1024 {
            return Err(std::io::Error::other("metadata exceeds bound"));
        }
        self.hash.update(&bytes[..count]);
        Ok(count)
    }
}

fn checksum(path: &Path) -> Result<String> {
    let mut reader = Hashed { inner: File::open(path)?, hash: Sha512::new(), bytes: 0 };
    std::io::copy(&mut reader, &mut std::io::sink())?;
    Ok(format!("{:x}", reader.hash.finalize()))
}

fn run(source: &Path, expected: &str, expected_open: &str, destination: &Path) -> Result<()> {
    if [expected, expected_open].iter().any(|hash| {
        hash.len() != 128
            || !hash.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err("expected complete lowercase SHA-512 digests".into());
    }
    if checksum(source)? != expected {
        return Err("compressed metadata checksum mismatch".into());
    }
    let decoder = zstd::Decoder::new(File::open(source)?)?;
    let hashed = Hashed { inner: decoder, hash: Sha512::new(), bytes: 0 };
    let mut reader = BufReader::new(hashed);
    let mut file = OpenOptions::new().write(true).create_new(true).open(destination)?;
    let count = index(&mut reader, &mut file)?;
    // Drain the decoder so the open checksum includes bytes after the XML end.
    std::io::copy(&mut reader, &mut std::io::sink())?;
    let hashed = reader.into_inner();
    if format!("{:x}", hashed.hash.finalize()) != expected_open {
        return Err("uncompressed metadata checksum mismatch; output is unverified".into());
    }
    file.sync_all()?;
    println!(
        "packages={count}\ncompressed_sha512={expected}\nopen_sha512={expected_open}\nindex=verified"
    );
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [mode, solver, source, destination] if mode == "solver-selection" => {
            solver_selection(Path::new(solver), Path::new(source), Path::new(destination))
        }
        [mode, source, destination, names @ ..] if mode == "select" && !names.is_empty() => {
            select(Path::new(source), Path::new(destination), names, false)
        }
        [mode, source, destination, names @ ..] if mode == "provides" && !names.is_empty() => {
            select(Path::new(source), Path::new(destination), names, true)
        }
        [source, expected, expected_open, destination] => {
            run(Path::new(source), expected, expected_open, Path::new(destination))
        }
        _ => {
            Err("usage: rpm-md-index PRIMARY_ZST EXPECTED_SHA512 EXPECTED_OPEN_SHA512 FRESH_JSONL"
                .into())
        }
    };
    if let Err(error) = result {
        eprintln!("RPM metadata index failed: {error}");
        std::process::exit(1);
    }
}

type PackageIdentity = (String, String, String);

fn solver_requests<R: BufRead>(input: R) -> Result<(BTreeSet<PackageIdentity>, u64)> {
    let mut reader = Reader::from_reader(input);
    let mut buffer = Vec::new();
    let mut requests = BTreeSet::new();
    let mut summary = None;
    let mut installing = false;
    let mut ended = false;
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) if element.name().as_ref() == b"install-summary" => {
                if summary.is_some() {
                    return Err("duplicate solver summary".into());
                }
                let attrs = attributes(&element)?;
                let count = attrs["packages-to-change"]
                    .as_str()
                    .ok_or("missing solver count")?
                    .parse::<usize>()?;
                let bytes = attrs["download-size"]
                    .as_str()
                    .ok_or("missing download size")?
                    .parse::<u64>()?;
                summary = Some((count, bytes));
            }
            Event::Start(element) if element.name().as_ref() == b"to-install" => installing = true,
            Event::Start(element) if element.name().as_ref() == b"solvable" => {
                let attrs = attributes(&element)?;
                if !installing || attrs["type"] != "package" || attrs["repository"] != "repo-oss" {
                    return Err("solver selected an unexpected operation or repository".into());
                }
                let name = attrs["name"].as_str().ok_or("missing solver package")?;
                let arch = attrs["arch"].as_str().ok_or("missing solver architecture")?;
                let edition = attrs["edition"].as_str().ok_or("missing solver edition")?;
                if !matches!(arch, "x86_64" | "noarch")
                    || name.is_empty()
                    || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"+_.-".contains(&b))
                    || !requests.insert((name.into(), arch.into(), edition.into()))
                {
                    return Err("invalid or duplicate solver package".into());
                }
            }
            Event::End(element) if element.name().as_ref() == b"to-install" => installing = false,
            Event::End(element) if element.name().as_ref() == b"stream" => ended = true,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let (count, bytes) = summary.ok_or("missing solver summary")?;
    if !ended || installing || requests.is_empty() || requests.len() != count {
        return Err("incomplete solver package closure".into());
    }
    Ok((requests, bytes))
}

fn match_solver<R: BufRead>(
    input: R,
    mut requests: BTreeSet<PackageIdentity>,
) -> Result<Vec<Value>> {
    let mut records = BTreeMap::new();
    for line in input.lines() {
        let record: Value = serde_json::from_str(&line?)?;
        let name = record["name"].as_str().ok_or("missing indexed name")?;
        let arch = record["arch"].as_str().ok_or("missing indexed architecture")?;
        let version = &record["version"];
        let epoch = version["epoch"].as_str().ok_or("missing indexed epoch")?;
        let edition = format!(
            "{}-{}",
            version["ver"].as_str().ok_or("missing indexed version")?,
            version["rel"].as_str().ok_or("missing indexed release")?
        );
        let edition = if epoch == "0" { edition } else { format!("{epoch}:{edition}") };
        let identity = (name.into(), arch.into(), edition);
        if requests.remove(&identity) {
            records.insert(identity, record);
        } else if records.contains_key(&identity) {
            return Err("duplicate exact indexed package".into());
        }
    }
    if !requests.is_empty() {
        return Err("solver package identity missing from signed index".into());
    }
    Ok(records.into_values().collect())
}

fn solver_selection(solver: &Path, source: &Path, destination: &Path) -> Result<()> {
    let (requests, bytes) = solver_requests(BufReader::new(File::open(solver)?))?;
    let records = match_solver(BufReader::new(File::open(source)?), requests)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(destination)?;
    serde_json::to_writer_pretty(&mut file, &records)?;
    file.write_all(b"\n")?;
    println!("selected_records={}\nexpected_download_bytes={bytes}", records.len());
    Ok(())
}

fn select(source: &Path, destination: &Path, names: &[String], by_provide: bool) -> Result<()> {
    let names: BTreeSet<_> = names.iter().map(String::as_str).collect();
    let mut found = BTreeSet::new();
    let mut records = Vec::new();
    for line in BufReader::new(File::open(source)?).lines() {
        let record: Value = serde_json::from_str(&line?)?;
        let name = record["name"].as_str().ok_or("missing indexed package name")?;
        let arch = record["arch"].as_str().ok_or("missing indexed package architecture")?;
        let matches: BTreeSet<_> = if by_provide {
            record["provides"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry["name"].as_str())
                .filter(|provide| names.contains(provide))
                .map(str::to_owned)
                .collect()
        } else if names.contains(name) {
            BTreeSet::from([name.to_owned()])
        } else {
            BTreeSet::new()
        };
        if !matches.is_empty() && matches!(arch, "x86_64" | "noarch") {
            found.extend(matches);
            records.push(record);
        }
    }
    if names.iter().any(|name| !found.contains(*name)) {
        return Err("requested package missing from signed index".into());
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(destination)?;
    serde_json::to_writer_pretty(&mut file, &records)?;
    file.write_all(b"\n")?;
    println!("matched_requests={}\nselected_records={}", found.len(), records.len());
    for record in records {
        println!(
            "{}\t{}:{}-{}\t{}\t{}:{}\t{}",
            record["name"].as_str().ok_or("missing name")?,
            record["version"]["epoch"].as_str().ok_or("missing epoch")?,
            record["version"]["ver"].as_str().ok_or("missing version")?,
            record["version"]["rel"].as_str().ok_or("missing release")?,
            record["arch"].as_str().ok_or("missing architecture")?,
            record["checksum_type"].as_str().ok_or("missing checksum type")?,
            record["checksum"].as_str().ok_or("missing checksum")?,
            record["location"]["href"].as_str().ok_or("missing location")?
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const METADATA: &str = "<metadata xmlns:rpm='rpm' packages='1'><package type='rpm'><name>qt6-example</name><arch>x86_64</arch><version epoch='0' ver='6.11.2' rel='1.1'/><checksum type='sha256'>aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</checksum><location href='x86_64/example.rpm'/><format><rpm:provides><rpm:entry name='qt6qmlimport(QtQuick3D)' flags='EQ' ver='6.11'/></rpm:provides><rpm:requires><rpm:entry name='libQt6Concurrent.so.6()(64bit)'/><rpm:entry name='version &gt;= 1'/></rpm:requires></format></package></metadata>";

    #[test]
    fn solver_selection_matches_full_signed_identity_and_refuses_other_operations() {
        let solver = "<stream><install-summary packages-to-change='1' download-size='123'><to-install><solvable type='package' name='qt6-example' arch='x86_64' edition='6.11.2-1.1' repository='repo-oss'></solvable></to-install></install-summary></stream>";
        let (requests, bytes) = solver_requests(solver.as_bytes()).unwrap();
        assert_eq!(bytes, 123);
        let mut indexed = Vec::new();
        index(METADATA.as_bytes(), &mut indexed).unwrap();
        assert_eq!(match_solver(indexed.as_slice(), requests.clone()).unwrap().len(), 1);
        assert!(
            match_solver(
                indexed.as_slice(),
                BTreeSet::from([("qt6-example".into(), "x86_64".into(), "6.11.1-1.1".into())])
            )
            .is_err()
        );
        for bad in [
            solver.replace("repo-oss", "foreign"),
            solver.replace("to-install", "to-remove"),
            solver.replace("x86_64", "aarch64"),
            solver.replace("packages-to-change='1'", "packages-to-change='2'"),
            solver.replace("</stream>", ""),
        ] {
            assert!(solver_requests(bad.as_bytes()).is_err());
        }
    }

    #[test]
    fn retains_exact_identity_relations_and_decodes_xml_entities() {
        let mut output = Vec::new();
        assert_eq!(index(METADATA.as_bytes(), &mut output).unwrap(), 1);
        let package: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(package["version"]["ver"], "6.11.2");
        assert_eq!(package["provides"][0]["name"], "qt6qmlimport(QtQuick3D)");
        assert_eq!(package["requires"][1]["name"], "version >= 1");
    }

    #[test]
    fn rejects_truncated_counts_duplicate_identities_and_unsupported_checksums() {
        for malformed in [
            METADATA.replace("packages='1'", "packages='2'"),
            METADATA.replace("</metadata>", ""),
            METADATA.replace("<arch>x86_64</arch>", "<arch>x86_64</arch><arch>noarch</arch>"),
            METADATA.replace("sha256", "md5"),
        ] {
            assert!(index(malformed.as_bytes(), &mut Vec::new()).is_err());
        }
    }
}
