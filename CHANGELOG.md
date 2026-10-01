# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/bddkit/bddkit/compare/v0.2.2...v0.3.0) - 2026-10-01

### Added

- accept a <<variable>> in a typed step position ([#66](https://github.com/bddkit/bddkit/pull/66))
- [**breaking**] macro templates are Cucumber Expressions ([#66](https://github.com/bddkit/bddkit/pull/66))
- declare builtin steps as Cucumber Expressions ([#66](https://github.com/bddkit/bddkit/pull/66))
- clean up after an interrupted run (issue #62)
- add curl/irm install scripts for GitHub Releases
- warn when a global variable write has no visible effect (issue #51)

### Fixed

- resolve nested I include paths relative to the including file (issue #76)
- abort the interrupt listener once a run finishes normally

### Other

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
