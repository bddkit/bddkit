# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.2](https://github.com/bddkit/bddkit/compare/v0.3.1...v0.3.2) - 2026-10-18

### Added

- bddkit report renders an event stream as one self-contained HTML page (bddkit/bddkit#110)
- step comments, disabled steps and sections in the event stream (bddkit/bddkit#109)
- the Cucumber JSON report carries descriptions, tags and the feature's line (bddkit/bddkit#108)
- descriptions, tags, rule and selection in the event stream (bddkit/bddkit#108)
- bddkit plugin prune removes plugin versions no lock references (bddkit/bddkit#124)
- bddkit upgrade moves the binary and its plugins together, with rollback (bddkit/bddkit#123)
- daily job that keeps the plugin index's ABI map current (bddkit/bddkit#122)

### Fixed

- run, validate and include scenarios written inside Rule: blocks (bddkit/bddkit#107)

### Other

- the report command, and a self-test report on every CI run (bddkit/bddkit#110)
- move report.rs to report/mod.rs (bddkit/bddkit#110)
- update the plugin index's ABI map
- the new event stream and report fields (bddkit/bddkit#108)
- read descriptions and tag lines from the source text (bddkit/bddkit#108)

## [0.3.1](https://github.com/bddkit/bddkit/compare/v0.3.0...v0.3.1) - 2026-10-16

### Fixed

- install and update only plugin releases built for the host's ABI (bddkit/bddkit#121)

## [0.3.0](https://github.com/bddkit/bddkit/compare/v0.2.2...v0.3.0) - 2026-10-13

### Added

- [**breaking**] plugin evidence hint, declared secrets and secret step groups (bddkit/bddkit#90)
- add run --evidence, resolved values, exchanges, statements and diagnostics in the stream (bddkit/bddkit#89)
- mask secrets in everything bddkit prints or writes (bddkit/bddkit#87)
- add run --artifacts-dir, one relocatable root for evidence files (bddkit/bddkit#88)
- add run --events, a line-delimited stream of run lifecycle events (bddkit/bddkit#86)
- add I attach the file step for multipart/form-data uploads (bddkit/bddkit#93)
- [**breaking**] exit 143 on SIGTERM and 129 on SIGHUP (bddkit/bddkit#85)
- add Print table: debug step with aligned columns (bddkit/bddkit#92)
- accept a <<variable>> in a typed step position ([#66](https://github.com/bddkit/bddkit/pull/66))
- [**breaking**] macro templates are Cucumber Expressions ([#66](https://github.com/bddkit/bddkit/pull/66))
- declare builtin steps as Cucumber Expressions ([#66](https://github.com/bddkit/bddkit/pull/66))
- clean up after an interrupted run (issue #62)
- add curl/irm install scripts for GitHub Releases
- warn when a global variable write has no visible effect (issue #51)

### Fixed

- hand install.sh off to install.ps1 on Windows and download with curl.exe (bddkit/bddkit#81)
- keep file parts in the multipart dump and check attached files in macro bodies (bddkit/bddkit#93)
- resolve nested I include paths relative to the including file (issue #76)
- abort the interrupt listener once a run finishes normally

### Other

- describe --evidence (bddkit/bddkit#89)
- add selftest/, bddkit run by bddkit through the exec plugin (bddkit/bddkit#87)
- specify and describe privacy masking (bddkit/bddkit#87)
- demonstrate file upload in examples with a variable path, extraction and several files (bddkit/bddkit#93)
- add appium to the plugin registry
- demonstrate typed parameters and typed-position variables in examples ([#66](https://github.com/bddkit/bddkit/pull/66))
- compile the builtin step table once ([#66](https://github.com/bddkit/bddkit/pull/66))
- step declarations as Cucumber Expressions, typed variables ([#66](https://github.com/bddkit/bddkit/pull/66))
- define a <<…>> slot once, as a group-free PLACEHOLDER

## [0.2.2](https://github.com/bddkit/bddkit/compare/v0.2.1...v0.2.2) - 2026-09-28

### Added

- bddkit plugin list, install, update, remove and show (issue #49)
- add call-site export prefix to I include (issue #52)
- log a macro's exported variables in debug mode (issue #53)
- run an included scenario inline, sharing World state, exporting declared variables
- detect an include cycle and cap include nesting at 16, recursively
- validate an include's target, scenario and with: table before the first request
- validate an include's with: table and build its concrete scenario
- resolve an include's target file and pick its scenario
- register the I include step patterns
- collect the Outline placeholders a scenario's steps use
- parse the @exports scenario tag
- show each database query's duration in debug mode (issue #54)
- default config file lookup (issue #48)
- assertions over a variable's text, and extract from a variable ([#39](https://github.com/bddkit/bddkit/pull/39))

### Fixed

- address final review findings for I include (with: syntax, docstring gap, error chains, docs accuracy)
- normalize feature-file paths to forward slashes in output (issue #56)

### Other

- document bddkit plugin commands, data directories and the release convention
- remove tempfile dependency, dedupe with: shape-checking, extract acceptance test helper
- document I include and its known limitations
- thread the current file's path through execute_step's recursion
- revert manual CHANGELOG edit — release-plz generates it
- check formatting (cargo fmt --check)
- cargo fmt (root + fixture plugins)
- Merge pull request #59 from sergeym/feat/issue-39-variable-assertions

## [0.2.1](https://github.com/bddkit/bddkit/compare/v0.2.0...v0.2.1) - 2026-09-20

### Added

- run --junit / --cucumber-json write machine-readable reports
- bddkit doctor honors --bddkit-dir and reports the lock chain
- --bddkit-dir / BDDKIT_DIR override for the lock chain
- read plugins.yaml through the layered directory chain
- base/local candidate files per bddkit layer
- layered .bddkit directory resolver
- *(doctor)* report a step whose resource group declares nothing (issue #42)
- implicit_instance in the plugin manifest (issue #37)
- assertions on a raw (non-JSON) response body (issue #4)
- JSON absence assertions (issue #22)

### Other

- the .bddkit layer chain, --bddkit-dir, plugins.local.yaml, and doctor's chain report

## [0.2.0](https://github.com/sergeym/bddkit/compare/v0.1.1...v0.2.0) - 2026-09-06

### Added

- `bddkit version`, and a `--version` that points somewhere
- `<<run_id>>` and condition operators, for run-scoped cleanup
- a plugin manifest declares the form of a field's value
- `bddkit resource add`, a validated resource written into the config
- `bddkit resource fields`, and the host side of the plugin config contract
- *(plugin)* describe a group's config fields and probe it live
- `bddkit doctor`, a suite check that does not need a run
- *(db)* fill insert result variables without RETURNING
- *(db)* pick the platform from the connection
- *(db)* the MySQL and MariaDB dialect
- *(plugin)* list plugin steps and their descriptions
- *(cli)* bddkit steps list
- *(steps)* step templates and translated descriptions
- *(steps)* group, description and named parameters for every step
- *(cli)* [**breaking**] run is now a subcommand

### Fixed

- *(steps)* usable help line, honest json filtering and plugin templates

### Other

- MySQL and MariaDB support
- *(db)* run every connection through sqlx::Any
- *(db)* move the SQL dialect behind a Platform trait
- leave the version and changelog to release-plz
- the steps command and the run split

## [0.1.1](https://github.com/sergeym/bddkit/compare/v0.1.0...v0.1.1) - 2026-08-27

### Added

- give each feature file a working directory
- *(plugin)* per_worker instances, one per feature file
- *(plugin)* load at startup, drop after the pool drains
- *(runner)* dispatch plugin steps off the executor
- *(plugin)* plugin steps, selected per scenario
- *(plugin)* eager config validation, lazy instances
- *(config)* opaque resource groups served by plugins
- *(plugin)* load a cdylib over a JSON ABI

### Other

- the plugin contract, and a plugin written against it
- *(plugin)* end-to-end acceptance for the plugin layer
- run the examples against a local Smocker mock server
