# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
