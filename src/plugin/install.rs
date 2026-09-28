//! What `bddkit plugin` fetches and unpacks: the index, a release's archive
//! and checksum, and the one library inside. Lock files and layers are
//! `manage.rs`'s business, not this module's.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// What `plugin install` was asked for: an index name or `owner/repo`, and a
/// release version without its `v` — `None` meaning the latest release.
#[derive(Debug, PartialEq, Eq)]
pub struct Spec {
    pub target: String,
    pub version: Option<String>,
}

impl Spec {
    pub fn parse(arg: &str) -> Result<Self> {
        let (target, version) = match arg.rsplit_once('@') {
            Some((target, version)) => {
                let version = version.strip_prefix('v').unwrap_or(version);
                check_segment("version", version)?;
                (target, Some(version.to_string()))
            }
            None => (arg, None),
        };
        if target.contains('/') {
            check_repo(target)?;
        } else if target.is_empty() {
            bail!("{arg:?} is neither a plugin name nor owner/repo");
        }
        Ok(Self {
            target: target.to_string(),
            version,
        })
    }

    pub fn is_repo(&self) -> bool {
        self.target.contains('/')
    }
}

/// One path segment — a directory name under `<data>/plugins/<repo>/<version>`,
/// which a reinstall or an uninstall later `remove_dir_all`s exactly
/// (`manage::remove_version_dir`) — so it can never be empty, `.`, `..`, or
/// carry a path separator of either kind. Restricted to ASCII
/// `[A-Za-z0-9._+-]` rather than merely refusing separators, so a Windows
/// drive prefix (`C:`) is refused on every host, not only on Windows: `:` is
/// simply not in the allowed set. `kind` names what was refused (`"version"`,
/// `"repository owner"`, …) in the error. Shared by `Spec::parse` (a version
/// or repo the caller typed), `latest_version` (a version GitHub's own release
/// tag names) and `manage::install_into` (the repo, from any of its three
/// origins — the argument, an index entry, or a lock entry's `source`) — one
/// gate all four go through, so none of them can hand a traversal to another.
pub(crate) fn check_segment(kind: &str, value: &str) -> Result<()> {
    let is_segment = !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
    if !is_segment {
        bail!("{value:?} is not a valid plugin {kind}: it must be a single path segment");
    }
    Ok(())
}

/// `owner/repo`, each half exactly one `check_segment`-valid segment — refuses
/// `owner/..`, `owner/.`, a half carrying `/` or `\`, and a third segment
/// (`a/b/c`, whose second half `b/c` is not itself a valid segment).
pub(crate) fn check_repo(repo: &str) -> Result<()> {
    let Some((owner, name)) = repo.split_once('/') else {
        bail!("{repo:?} is not a valid plugin repository: it must be owner/repo");
    };
    check_segment("repository owner", owner).with_context(|| format!("in {repo:?}"))?;
    check_segment("repository name", name).with_context(|| format!("in {repo:?}"))?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    pub repo: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Deserialize)]
struct Index {
    #[serde(default)]
    plugin: Vec<IndexEntry>,
}

pub fn parse_index(raw: &str) -> Result<Vec<IndexEntry>> {
    Ok(serde_yaml_ng::from_str::<Index>(raw)
        .context("malformed plugin index")?
        .plugin)
}

/// Case-insensitive substring over name and description; `None` is everything.
pub fn search<'a>(index: &'a [IndexEntry], query: Option<&str>) -> Vec<&'a IndexEntry> {
    let query = query.map(str::to_lowercase);
    index
        .iter()
        .filter(|entry| match &query {
            None => true,
            Some(q) => {
                entry.name.to_lowercase().contains(q)
                    || entry.description.to_lowercase().contains(q)
            }
        })
        .collect()
}

/// The release target this host loads: one of the five triples the release
/// workflows build. A host outside them has nothing to download.
pub fn host_target() -> Result<&'static str> {
    Ok(match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "windows") => "x86_64-pc-windows-msvc",
        (arch, os) => bail!("no plugin releases are published for {arch}-{os}"),
    })
}

/// Release assets are named after the repository, not the plugin:
/// `bddkit/bddkit-exec` publishes `bddkit-exec-v…`.
pub fn repo_basename(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

pub fn asset_name(repo: &str, version: &str, target: &str) -> String {
    let ext = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("{}-v{version}-{target}.{ext}", repo_basename(repo))
}

/// A `.sha256` asset is `sha256sum` output — `<hex>  <file name>` — and only
/// the hex is compared. Returns it lower-cased, for the lock entry. It comes
/// from the same release as the archive, so it proves the download is intact,
/// not who built it.
pub fn verify_sha256(archive: &[u8], sha_file: &str) -> Result<String> {
    let expected = sha_file
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let actual = format!("{:x}", Sha256::digest(archive));
    if expected != actual {
        bail!("checksum mismatch: the release says {expected}, the download is {actual}");
    }
    Ok(actual)
}

/// Writes the one dynamic library in `archive` into `dest` under its own file
/// name and nowhere else: the archive's directory part is dropped, so an entry
/// named `../../x.so` cannot land outside `dest`. Zero or several libraries is
/// a malformed release, refused rather than guessed at.
pub fn extract_library(archive: &[u8], asset: &str, dest: &Path) -> Result<PathBuf> {
    let found = if asset.ends_with(".zip") {
        zip_libraries(archive)
    } else {
        tar_libraries(archive)
    }
    .with_context(|| format!("failed to read {asset}"))?;
    let (file, bytes) = match <[_; 1]>::try_from(found) {
        Ok([one]) => one,
        Err(found) => bail!(
            "{asset} must hold exactly one plugin library, found {}: {:?}",
            found.len(),
            found.iter().map(|(name, _)| name).collect::<Vec<_>>()
        ),
    };
    std::fs::create_dir_all(dest)
        .with_context(|| format!("failed to create {}", dest.display()))?;
    let path = dest.join(file);
    std::fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// The file-name part of an archive entry, if it names this host's kind of
/// dynamic library (`lib*.so`, `lib*.dylib`, `*.dll`).
fn library_name(entry: &str) -> Option<String> {
    let file = Path::new(entry).file_name()?.to_str()?;
    (file.starts_with(std::env::consts::DLL_PREFIX) && file.ends_with(std::env::consts::DLL_SUFFIX))
        .then(|| file.to_string())
}

fn tar_libraries(archive: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        if let Some(name) = library_name(&entry.path()?.to_string_lossy()) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            out.push((name, bytes));
        }
    }
    Ok(out)
}

fn zip_libraries(archive: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))?;
    let mut out = Vec::new();
    for index in 0..zip.len() {
        let mut file = zip.by_index(index)?;
        let name = match library_name(file.name()) {
            Some(name) if file.is_file() => name,
            _ => continue,
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        out.push((name, bytes));
    }
    Ok(out)
}

pub const DEFAULT_REGISTRY: &str =
    "https://raw.githubusercontent.com/bddkit/bddkit/main/plugin-registry.yaml";
pub const DEFAULT_GITHUB: &str = "https://github.com";

/// Where the index and the releases come from. Both are overridable — tests
/// point them at a stub, a mirror or GitHub Enterprise points them elsewhere.
pub struct Sources {
    pub registry: String,
    pub github: String,
}

impl Sources {
    pub fn from_env() -> Self {
        let var = |name: &str, default: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| default.to_string())
        };
        Self {
            registry: var("BDDKIT_PLUGIN_REGISTRY", DEFAULT_REGISTRY),
            github: var("BDDKIT_GITHUB_URL", DEFAULT_GITHUB)
                .trim_end_matches('/')
                .to_string(),
        }
    }
}

async fn get(client: &reqwest::Client, url: &str) -> Result<reqwest::Response> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !response.status().is_success() {
        bail!("GET {url}: {}", response.status());
    }
    Ok(response)
}

pub async fn fetch_index(client: &reqwest::Client, sources: &Sources) -> Result<Vec<IndexEntry>> {
    let raw = get(client, &sources.registry).await?.text().await?;
    parse_index(&raw).with_context(|| format!("plugin index {}", sources.registry))
}

/// The release tag named by a `releases/latest` redirect's landed URL — the
/// last of exactly `…/releases/tag/<tag>`, read through `Url::path_segments`
/// rather than `str::split_once` over the whole URL: the raw string also
/// carries the query and fragment, where a redirect landing at
/// `…/releases/tag/v1.2.3?ref=heads/main` would otherwise fold the query
/// straight into the tag. `None` means the shape does not match at all — no
/// published release. A trailing slash leaves one empty path segment, popped
/// before matching.
fn tag_from_redirect(url: &reqwest::Url) -> Option<&str> {
    let mut segments: Vec<&str> = url.path_segments()?.collect();
    if segments.last() == Some(&"") {
        segments.pop();
    }
    match segments.as_slice() {
        [.., "releases", "tag", tag] => Some(*tag),
        _ => None,
    }
}

/// GitHub answers `releases/latest` with a redirect to `releases/tag/<tag>`;
/// reading the tag off the final URL costs no API call and no rate limit.
pub async fn latest_version(client: &reqwest::Client, github: &str, repo: &str) -> Result<String> {
    let url = format!("{github}/{repo}/releases/latest");
    let response = get(client, &url).await?;
    let landed = response.url().clone();
    let tag = tag_from_redirect(&landed)
        .with_context(|| format!("{repo} has no published release ({url} led to {landed})"))?;
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    check_segment("version", &version).with_context(|| format!("{repo} released tag {tag:?}"))?;
    Ok(version)
}

pub struct Download {
    pub asset: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

/// The archive for `target` and its verified checksum. A missing asset — no
/// such version, or no build for this platform — is the 404 naming its URL.
pub async fn download(
    client: &reqwest::Client,
    github: &str,
    repo: &str,
    version: &str,
    target: &str,
) -> Result<Download> {
    let asset = asset_name(repo, version, target);
    let url = format!("{github}/{repo}/releases/download/v{version}/{asset}");
    let bytes = get(client, &url).await?.bytes().await?.to_vec();
    let sha_file = get(client, &format!("{url}.sha256")).await?.text().await?;
    let sha256 = verify_sha256(&bytes, &sha_file).with_context(|| asset.clone())?;
    Ok(Download {
        asset,
        bytes,
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn lib(stem: &str) -> String {
        format!(
            "{}{stem}{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        )
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
        let dir =
            std::env::temp_dir().join(format!("bddkit-install-{name}-{}", std::process::id()));
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
}
