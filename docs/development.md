# Разработка и сборка

Инструкции по установке готового приложения находятся в [README](../README.md). Этот документ предназначен для сборки из исходного кода.

## Требования

- Rust 1.99.1 из `rust-toolchain.toml`; версии зависимостей закреплены в `Cargo.lock`.
- Windows: Visual Studio Build Tools с C++ toolchain и Windows SDK.
- Linux/WSL: для кросс-сборки Windows нужны MinGW и Rust target `x86_64-pc-windows-gnu`. Захват экрана и Windows OCR в Linux недоступны.
- Для установщика — NSIS 3; для подготовки стартовой базы — Python 3.

## Проверки и EXE

Из корня репозитория:

```sh
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

В Windows те же проверки и сборку выполняет:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build_windows.ps1
```

Результат: `target/release/eve-combat-assistant.exe`. EXE можно запустить без стартовой базы, но общие варианты появятся только после загрузки данных. Для проверки интерфейса без сети используйте `--demo --data-dir` с отдельной тестовой папкой.

Кросс-сборка из Linux/WSL:

```sh
rustup target add x86_64-pc-windows-gnu
cargo clippy --locked --all-targets --target x86_64-pc-windows-gnu -- -D warnings
cargo build --release --locked --target x86_64-pc-windows-gnu
```

В `.cargo/config.toml` задано одно задание, LTO отключено в release-профиле. Это ограничивает пик памяти компиляции и линковки.

## Стартовая база и дистрибутив

`scripts/build_bootstrap.py` собирает базу из публичного дневного архива zKillboard и подготавливает локальные группировки всех корпусов. Полная загрузка занимает время и требует сети и свободного диска:

```sh
python3 scripts/build_matchups.py --raw-directory build/raw-cache --directory build/matchups --end 2026-10-29 --days 365
python3 scripts/build_bootstrap.py --directory build/seed --end 2026-10-29 --days 90 --prepare-executable target/release/eve-combat-assistant
python3 scripts/verify_matchups.py build/matchups --raw-directories build/seed/raw build/matchups/raw build/raw-cache --totals build/seed/totals.json
python3 scripts/calibrate_ratings.py build/matchups
python3 scripts/build_bootstrap.py --directory build/seed --end 2026-10-29 --days 90 --matchup-directory build/matchups
```

Доступные параметры и пути выводятся через `--help`. Результат — каталог с `seed.sqlite3` и `seed-manifest.json`. Скрипт упаковки проверяет версию схемы, SDE, полноту периода, группировки, годовую статистику корпусов и SHA256 базы. Для другого среза укажите последний полный день UTC. Скрипт годовой статистики возобновляет сбор по готовым дням и читает JSON по одной записи; архивы остаются только в каталоге сборки. `verify_matchups.py` сверяет дневные итоги и индексы, сохраняет дополнительные raw-записи и восстанавливает индексированные пропуски по публичным ESI hash.

Для актуализации существующего снимка используйте `build_bootstrap.py --refresh-period`: перекрывающиеся полные дни переиспользуются, из периода выходят старые даты, новые и изменённые дни дозагружаются. После проверки обновлённой годовой истории повторите калибровку и присоединение таблиц. Клиенту поставляются только компактные дневные суммы. Его обновление пересчитывает винрейты и цвета по локальной базе, отдельную годовую загрузку не запускает.

Для упаковки также нужен каталог текстов лицензий зависимостей с `DEPENDENCIES.json`. Список требований и лицензий — в [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md). Само приложение и комплект лицензий должны соответствовать одному `Cargo.lock`.

```powershell
powershell -ExecutionPolicy Bypass -File scripts/package_windows.ps1 -SeedDirectory C:\build\seed -LicenseDirectory C:\build\licenses
powershell -ExecutionPolicy Bypass -File scripts/build_installer.ps1
```

`package_windows.ps1` принимает необязательный `-Executable`. Он создаёт portable-архив в `dist`, затем `build_installer.ps1` собирает установщик из той же папки приложения.

## Диагностические команды

```text
eve-combat-assistant.exe --analyze --ship Rifter --fit my-fit.eft
eve-combat-assistant.exe --sync --ship Rifter --days 90
eve-combat-assistant.exe --archive-from 2026-10-01 --archive-to 2026-10-26
```

`--sync` загружает отдельный корпус через месячные страницы API; при достижении лимита сообщает о неполной загрузке. Дневной архив охватывает все корпуса, сохраняет контрольные точки и используется кнопкой обновления в интерфейсе. Для тестов задавайте отдельный `--data-dir`.

`scripts/windows_smoke.ps1` проверяет native OCR, запуск, снимки окон и CPU в простое. Основное окно в Linux headless можно проверить через Xvfb и `--demo --screenshot путь.png`; снимок окна настроек и OCR требуют Windows.

## Код и источники данных

Точка входа — `rust/main.rs`, устройство рабочих потоков описано в [архитектуре](architecture.md). Расчётчик EVEShipFit закреплён в `native/vendor/`, локальные изменения перечислены в `native/upstream.json`. SDE и справочник имён встроены в EXE; версии и контрольные суммы находятся в `native/upstream.json` и `native/data/SHA256SUMS`.

Источники: [zKillboard API](https://zkillboard.com/api/docs/), [ESI OpenAPI](https://esi.evetech.net/meta/openapi.json), [EVEShipFit dogma-engine](https://github.com/EVEShipFit/dogma-engine).
