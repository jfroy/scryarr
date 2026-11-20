# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.3](https://github.com/jfroy/scryarr/compare/v0.2.2...v0.2.3) - 2026-11-20

### Other

- update Cargo.lock dependencies

## [0.2.2](https://github.com/jfroy/scryarr/compare/v0.2.1...v0.2.2) - 2026-11-20

### Fixed

- *(deps)* keep rustls on ring and guard against new native builds
- *(deps)* update rust crate reqwest to 0.13 ([#7](https://github.com/jfroy/scryarr/pull/7))
- *(deps)* update rust crate sqlx to 0.9 ([#8](https://github.com/jfroy/scryarr/pull/8))
- *(deps)* update rust crate graphql_client to 0.16 ([#6](https://github.com/jfroy/scryarr/pull/6))
- *(deps)* update rust crate base64 to 0.23 ([#5](https://github.com/jfroy/scryarr/pull/5))

### Other

- use a ReDoS-safe exclude pattern for ghcr cleanup
- prune non-release images from ghcr
- stop publishing images for main commits
- tag latest only on releases
- find the release PR from release-plz's output
- *(deps)* update sigstore/cosign-installer action to v4.1.3 ([#1](https://github.com/jfroy/scryarr/pull/1))
- *(deps)* update rust docker tag to v1.100.0 ([#12](https://github.com/jfroy/scryarr/pull/12))
- *(deps)* update docker/dockerfile docker tag to v1.29 ([#2](https://github.com/jfroy/scryarr/pull/2))

## [0.2.1](https://github.com/jfroy/scryarr/compare/v0.2.0...v0.2.1) - 2026-11-20

### Other

- let auto-merge wait for the required test check
- release with release-plz instead of a hand-rolled script
- automerge non-major dependency updates and release weekly
