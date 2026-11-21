use super::*;
use std::io::Write;

#[tokio::test]
async fn the_range_picks_the_highest_abi_key_as_a_number() {
    let entry: IndexEntry = serde_yaml_ng::from_str(
        "name: x\nrepo: o/x\nabi: {\"3\": 0.3.0, \"9\": 0.9.0, \"10\": 0.10.0}\n",
    )
    .expect("entry");
    let client = reqwest::Client::new();
    let pick = |abis| host_release(&client, "http://unused", "o/x", Some(&entry), abis);
    assert_eq!(pick(3..=10).await.unwrap().unwrap(), "0.10.0");
    assert_eq!(pick(3..=9).await.unwrap().unwrap(), "0.9.0");
    assert_eq!(pick(3..=3).await.unwrap().unwrap(), "0.3.0");
    let refused = pick(4..=8).await.unwrap().unwrap_err();
    assert!(refused.contains("ABI 4 to 8"), "{refused}");
}

fn lib(stem: &str) -> String {
    dll_name(stem)
}

fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(gz);
    for (path, data) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, path, *data)
            .expect("append");
    }
    builder.into_inner().expect("tar").finish().expect("gz")
}

fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (path, data) in files {
        writer
            .start_file(*path, zip::write::SimpleFileOptions::default())
            .expect("start");
        writer.write_all(data).expect("write");
    }
    writer.finish().expect("zip").into_inner()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bddkit-install-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_spec_is_a_name_or_owner_repo_with_an_optional_version() {
    let parse = |arg| Spec::parse(arg).expect(arg);
    assert_eq!(
        parse("exec"),
        Spec {
            target: "exec".into(),
            version: None
        }
    );
    assert_eq!(parse("exec@0.2.0").version.as_deref(), Some("0.2.0"));
    assert_eq!(parse("exec@v0.2.0").version.as_deref(), Some("0.2.0"));
    let repo = parse("bddkit/bddkit-exec@v1.2.3");
    assert!(repo.is_repo());
    assert_eq!(repo.target, "bddkit/bddkit-exec");
    assert!(!parse("exec").is_repo());
    for bad in [
        "",
        "exec@",
        "exec@v",
        "/x",
        "x/",
        "a/b/c",
        "exec@..",
        "exec@.",
        "exec@1/2",
        "exec@1\\2",
        "owner/..",
        "owner/.",
        "a/b\\c",
    ] {
        assert!(Spec::parse(bad).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn a_version_must_be_exactly_one_path_segment() {
    for good in ["0.1.0", "v1", "1.2.3-rc1", "1.0.0+build.1", "1.0.0-rc.1"] {
        check_segment("version", good).expect(good);
    }
    for bad in ["", ".", "..", "1/2", "1\\2", "C:"] {
        let error = check_segment("version", bad).expect_err(bad);
        assert!(
            format!("{error:#}").contains("not a valid plugin version"),
            "{error:#}"
        );
    }
}

#[test]
fn a_repo_is_exactly_two_valid_segments() {
    check_repo("bddkit/bddkit-exec").expect("accepted");
    for bad in ["owner/..", "owner/.", "a/b\\c", "a/b/c", "/x", "x/", "solo"] {
        let error = check_repo(bad).expect_err(bad);
        assert!(
            format!("{error:#}").contains("not a valid plugin repository"),
            "{error:#}"
        );
    }
}

#[test]
fn the_index_is_searched_by_name_and_description_ignoring_case() {
    let index = parse_index(
        "plugin:\n  - name: exec\n    repo: bddkit/bddkit-exec\n    description: Run local commands\n  - name: s3\n    repo: bddkit/bddkit-s3\n",
    )
    .expect("parses");
    assert_eq!(index[1].description, "", "description is optional");
    let names = |q| {
        search(&index, q)
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(None), ["exec", "s3"]);
    assert_eq!(names(Some("COMMAND")), ["exec"]);
    assert_eq!(names(Some("S3")), ["s3"]);
    assert!(names(Some("mail")).is_empty());
}

#[test]
fn asset_names_follow_the_release_convention() {
    assert_eq!(
        asset_name("bddkit/bddkit-exec", "0.1.0", "x86_64-unknown-linux-gnu"),
        "bddkit-exec-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(
        asset_name("someone/widget", "2.0.0", "x86_64-pc-windows-msvc"),
        "widget-v2.0.0-x86_64-pc-windows-msvc.zip"
    );
}

#[test]
fn the_release_tag_is_read_from_path_segments_not_the_whole_url() {
    let clean = reqwest::Url::parse("https://github.com/o/r/releases/tag/v1.2.3").expect("url");
    assert_eq!(tag_from_redirect(&clean), Some("v1.2.3"));

    // A query string is not part of the path: the old `str::split_once`
    // over the whole URL folded it straight into the tag.
    let with_query =
        reqwest::Url::parse("https://github.com/o/r/releases/tag/v1.2.3?ref=heads/main")
            .expect("url");
    assert_eq!(tag_from_redirect(&with_query), Some("v1.2.3"));

    let trailing_slash =
        reqwest::Url::parse("https://github.com/o/r/releases/tag/v1.2.3/").expect("url");
    assert_eq!(tag_from_redirect(&trailing_slash), Some("v1.2.3"));

    let no_release = reqwest::Url::parse("https://github.com/o/r").expect("url");
    assert_eq!(tag_from_redirect(&no_release), None);
}

#[test]
fn the_checksum_file_is_sha256sum_output() {
    let hex = format!("{:x}", Sha256::digest(b"archive"));
    let file = format!("{}  x.tar.gz\n", hex.to_uppercase());
    assert_eq!(verify_sha256(b"archive", &file).expect("matches"), hex);
    let error = verify_sha256(b"tampered", &file).expect_err("mismatch");
    assert!(
        format!("{error:#}").contains("checksum mismatch"),
        "{error:#}"
    );
}

#[test]
fn the_one_library_is_extracted_under_its_own_name_only() {
    let dest = temp("tar-one");
    let archive = tar_gz(&[
        ("pkg/README.md", b"readme"),
        (&format!("pkg/nested/{}", lib("widget")), b"ELF"),
    ]);
    let path = extract_library(&archive, "w.tar.gz", &dest).expect("extracts");
    assert_eq!(
        path,
        dest.join(lib("widget")),
        "the directory part is dropped"
    );
    assert_eq!(std::fs::read(&path).expect("read"), b"ELF");
    assert!(!dest.join("README.md").exists(), "nothing else is unpacked");
}

#[cfg(unix)]
#[test]
fn the_bddkit_executable_is_extracted_and_made_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dest = temp("tar-bin");
    let archive = tar_gz(&[
        ("bddkit-v1-t/README.md", b"readme"),
        ("bddkit-v1-t/bddkit", b"#!/bin/sh"),
        (&format!("bddkit-v1-t/{}", lib("widget")), b"ELF"),
    ]);
    let path = extract_binary(&archive, "b.tar.gz", &dest).expect("extracts");
    assert_eq!(path, dest.join("bddkit"));
    assert_eq!(std::fs::read(&path).expect("read"), b"#!/bin/sh");
    let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
    assert_eq!(mode & 0o777, 0o755, "fs::write drops the archive's mode");
}

#[test]
fn zero_or_two_libraries_is_a_malformed_release() {
    let none = tar_gz(&[("pkg/README.md", b"readme")]);
    let error = extract_library(&none, "w.tar.gz", &temp("tar-none")).expect_err("none");
    assert!(format!("{error:#}").contains("found 0"), "{error:#}");
    let two = tar_gz(&[(&lib("a"), b"1"), (&lib("b"), b"2")]);
    let error = extract_library(&two, "w.tar.gz", &temp("tar-two")).expect_err("two");
    assert!(format!("{error:#}").contains("found 2"), "{error:#}");
}

#[test]
fn a_release_without_a_library_names_the_files_it_did_hold() {
    let none = tar_gz(&[("pkg/README.md", b"readme")]);
    let error = extract_library(&none, "w.tar.gz", &temp("tar-held")).expect_err("none");
    assert!(format!("{error:#}").contains("pkg/README.md"), "{error:#}");
    let none = zip_of(&[("pkg/README.md", b"readme")]);
    let error = extract_library(&none, "w.zip", &temp("zip-held")).expect_err("none");
    assert!(format!("{error:#}").contains("pkg/README.md"), "{error:#}");
}

#[test]
fn a_zip_asset_is_read_as_zip() {
    let dest = temp("zip");
    let archive = zip_of(&[
        ("pkg/LICENSE", b"l"),
        (&format!("pkg/{}", lib("widget")), b"PE"),
    ]);
    let path = extract_library(&archive, "w.zip", &dest).expect("extracts");
    assert_eq!(std::fs::read(path).expect("read"), b"PE");
}

#[test]
fn a_zip_entry_name_carrying_traversal_still_lands_inside_dest() {
    // The zip writer accepts a `../` entry name outright (unlike
    // `tar::Builder::append_data`, which refuses `..` up front) — the
    // spec calls for this case explicitly, since it is the one archive
    // format that can actually produce it.
    let dest = temp("zip-traversal");
    let archive = zip_of(&[(&format!("../../{}", lib("widget")), b"PE")]);
    let path = extract_library(&archive, "w.zip", &dest).expect("extracts");
    assert_eq!(
        path,
        dest.join(lib("widget")),
        "the directory part, traversal included, is dropped"
    );
    assert_eq!(std::fs::read(&path).expect("read"), b"PE");
    assert!(
        path.starts_with(&dest),
        "nothing is written outside dest: {}",
        path.display()
    );
    // The parent of `dest` (where `../../<lib>` would land uncorrected)
    // must not have gained a file either.
    let outside = dest
        .parent()
        .and_then(Path::parent)
        .map(|p| p.join(lib("widget")));
    if let Some(outside) = outside {
        assert!(
            !outside.exists(),
            "nothing escaped dest: {}",
            outside.display()
        );
    }
}

fn entry(name: &str, groups: &[&str]) -> IndexEntry {
    IndexEntry {
        name: name.into(),
        repo: format!("bddkit/bddkit-{name}"),
        description: String::new(),
        abi: BTreeMap::new(),
        groups: groups
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
    }
}

fn names(groups: &[&str]) -> Vec<String> {
    groups
        .iter()
        .map(std::string::ToString::to_string)
        .collect()
}

#[test]
fn a_group_maps_to_the_entry_that_claims_it_unless_that_one_is_installed() {
    let index = [entry("s3", &["s3"]), entry("api", &["api", "srp"])];
    let wanted = needs(&names(&["srp", "s3", "api"]), &names(&["s3"]), &index).unwrap();
    assert_eq!(
        wanted.missing.len(),
        1,
        "api once, though two groups name it"
    );
    assert_eq!(wanted.missing[0].name, "api");
    assert!(wanted.unclaimed.is_empty());
    let again = needs(&names(&["s3"]), &names(&["s3"]), &index).unwrap();
    assert!(again.missing.is_empty() && again.unclaimed.is_empty());
}

#[test]
fn a_group_nothing_in_the_index_claims_is_reported_not_guessed() {
    let index = [entry("s3", &["s3"])];
    let wanted = needs(&names(&["widget"]), &[], &index).unwrap();
    assert!(wanted.missing.is_empty());
    assert_eq!(wanted.unclaimed, ["widget"]);
}

#[test]
fn a_group_claimed_twice_is_refused() {
    let index = [entry("a", &["x"]), entry("b", &["x"])];
    let error = needs(&names(&["x"]), &[], &index).unwrap_err();
    assert!(
        format!("{error}").contains("both claim the group \"x\""),
        "{error}"
    );
}

#[test]
fn one_entry_listing_a_group_twice_is_not_a_duplicate_claim() {
    check_unique_groups(&[entry("a", &["x", "x"])]).unwrap();
}

#[test]
fn the_shipped_index_claims_no_group_twice() {
    let index = parse_index(include_str!("../../../plugin-registry.yaml")).unwrap();
    check_unique_groups(&index).unwrap();
}
