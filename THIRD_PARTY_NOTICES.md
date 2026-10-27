# Third-party notices

EVE Combat Assistant is an independent application. EVE Online and all related game data, ship images, names, and trademarks belong to CCP hf. This application is not endorsed by CCP. The bundled game-data license is in `licenses/EVE-DATA-LICENSE.txt` in the Windows package, and `native/data/LICENSE.EVE` in the source tree.

The in-process dogma evaluator is EVEShipFit dogma-engine, MIT, copyright EVEShipFit Team. Its exact revision is recorded in `native/upstream.json`; the vendored source and license are in `native/vendor/dogma-engine`. The SDE/name files are distributed by EVEShipFit (build 3503375); their notices and hashes are in `native/data`.

The desktop UI uses egui/eframe, under MIT OR Apache-2.0. Windows bindings, tray-icon, image codecs, networking, compression, JSON handling, SQLite bindings, and other Rust dependencies retain their respective licenses. SQLite itself is in the public domain. DejaVu Sans is used to render test fixtures and installer artwork; no DejaVu font files are bundled; application fonts are supplied by egui and covered by its bundled font notices.

The packaged `licenses/DEPENDENCIES.json` lists exact resolved dependency versions and SPDX declarations. Adjacent per-crate directories contain available upstream license and notice files, including bundled font licenses. This directory also includes the EVEShipFit and CCP data notices. See `Cargo.lock` for the complete source dependency resolution.

Public loss records are obtained from zKillboard and the official EVE ESI API. The embedded demonstration dataset is a small set of public Rifter loss records; no account credentials or OAuth tokens are bundled.
