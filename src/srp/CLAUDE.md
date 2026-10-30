# SRP

Loaded on demand; the root `CLAUDE.md` holds the invariants that apply everywhere.

- **SRP `hex-string` pads the client proof before hashing it into the server proof.** (`M1`/`M2` below are the SRP-6a proof messages, nothing to do with any release numbering.) The variant zero-pads the client proof `M1` to the hash's digest width before it goes into the server proof `M2`; getting that padding wrong — e.g. stripping the proof's leading zero the way some browser SRP clients do — silently produces a different `M2` about 1 login in 16, whenever the client proof's leading nibble happens to be zero.
