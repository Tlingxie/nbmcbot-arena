# Third-party notices

This file records known third-party material and the fixed upstream references
used by NBMCBot. It does not replace the licenses of Cargo dependencies or claim
to be a complete binary-distribution license manifest.

## Azalea

NBMCBot depends on Azalea `0.15.1+mc1.21.11`. The packet and empty-chunk builders in
`crates/nbmcbot/tests/support/mod.rs` were adapted from Azalea's
`default_login_packet` and `make_basic_empty_chunk` test helpers in
`azalea-client/src/test_utils/simulation.rs`.

The root `azalea` crate, `azalea-client`, `azalea-entity`, and `azalea-physics` are vendored under `vendor/` with
the upstream MIT license text. Local lifecycle and per-account cancellation
patches, source provenance and the original client package checksum are recorded
in [UPSTREAM-PATCHES](docs/UPSTREAM-PATCHES.md).

The local elytra and attached-firework movement calculations were independently
implemented in Rust after checking the observable arithmetic in the official
Minecraft Java 1.21.11 server bytecode. The official server and mappings are
runtime reference artifacts only and are not included in distributed source.
Minecraft remains the property of Mojang/Microsoft; the project's MIT declaration
does not relicense their server or mapping files. See the patch record for the
exact reference artifact hashes and methods.

- Upstream: [azalea-rs/azalea](https://github.com/azalea-rs/azalea).
- Published crate: [azalea 0.15.1+mc1.21.11](https://crates.io/crates/azalea/0.15.1+mc1.21.11).
- VCS revision recorded by the crate: `675e56fd9d9da084b8f923fdab6feda3ebd19568`.
  The published `.cargo_vcs_info.json` also records `dirty: true`; the revision
  alone is not a guarantee that every published source byte equals the checkout.
- Source helpers: [simulation.rs at the recorded revision](https://github.com/azalea-rs/azalea/blob/675e56fd9d9da084b8f923fdab6feda3ebd19568/azalea-client/src/test_utils/simulation.rs).
- License: [LICENSE.md at the recorded revision](https://github.com/azalea-rs/azalea/blob/675e56fd9d9da084b8f923fdab6feda3ebd19568/LICENSE.md).

The original copyright and permission notice follows. Retain it with copies or
substantial portions of the adapted Azalea material.

```text
MIT License

Copyright (c) 2022 mat

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Meteor Client: feature-reference baseline

The feature inventory was extracted from the official `1.21.11` branch pinned at
`48030717dc0d4898a195a713fb048bf597fc5b54`. The inventory records names, classes,
registration locations and setting identifiers; this documentation change does
not import Meteor's Java implementations into NBMCBot.

- Upstream: [MeteorDevelopment/meteor-client](https://github.com/MeteorDevelopment/meteor-client).
- Source attribution in module headers: Copyright (c) Meteor Development.
- License: [GNU GPL version 3](https://github.com/MeteorDevelopment/meteor-client/blob/48030717dc0d4898a195a713fb048bf597fc5b54/LICENSE).
- Upstream distribution guidance: [README licensing section](https://github.com/MeteorDevelopment/meteor-client/blob/48030717dc0d4898a195a713fb048bf597fc5b54/README.md#licensing).

Any future code adaptation or translation from Meteor must retain its source
attribution and satisfy the applicable GPL terms. A Rust rewrite is not by itself
permission to relicense copied or adapted implementation code as MIT. Record
the originating files and changes when such a port is added.

## Baritone: feature-reference baseline

The feature inventory was extracted from official tag `v1.17.0`, commit
`23723891da460ef15797b02fe5b385b0c5b163cc`, for Minecraft 1.21.11. This documentation
change does not import Baritone's Java implementations into NBMCBot.

- Upstream: [cabaletta/baritone](https://github.com/cabaletta/baritone).
- Release: [v1.17.0](https://github.com/cabaletta/baritone/releases/tag/v1.17.0).
- License: [GNU LGPL version 3](https://github.com/cabaletta/baritone/blob/23723891da460ef15797b02fe5b385b0c5b163cc/LICENSE).
- The Java [source headers](https://github.com/cabaletta/baritone/blob/23723891da460ef15797b02fe5b385b0c5b163cc/src/main/java/baritone/command/defaults/GotoCommand.java#L1) permit version 3 or a later version.
- The [README](https://github.com/cabaletta/baritone/blob/23723891da460ef15797b02fe5b385b0c5b163cc/README.md) also labels the license as having an "anime exception"; the repository contains [LICENSE-Part-2.jpg](https://github.com/cabaletta/baritone/blob/23723891da460ef15797b02fe5b385b0c5b163cc/LICENSE-Part-2.jpg), an image with the words "No anime". This notice records those upstream artifacts without inventing an SPDX exception or additional license text.

Future source ports must preserve the applicable notices and license terms, with
the original files and modifications recorded. The NBMCBot package-level MIT
field does not override the license of imported Baritone material.

## Project declaration and remaining distribution work

NBMCBot's Cargo manifests currently declare MIT for its own packages. The Azalea
notice above is still necessary for the adapted fixture; a package license field
or an attribution comment alone does not include the upstream permission text.
This notice does not choose a copyright holder for NBMCBot's original code.

This review covered the known Azalea fixture adaptation and the two pinned
feature-reference repositories. It did not audit every transitive Cargo
dependency. A redistributed executable must carry the notices required by its
actual dependency set and any source ports included at that time. Addon
repositories and versions have not yet been selected, so their licenses and
notices remain unenumerated.
