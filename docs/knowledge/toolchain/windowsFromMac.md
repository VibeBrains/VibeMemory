# Сборка под Windows на Mac: цель `gnu`, а не `msvc` (2026-09-27)

План этапа 6 делал GPD сборочной машиной под Windows: Rust, MSVC Build Tools и `releaseBuild.ps1`.
Rust на GPD так и не поставили, и первый Windows-архив к выпуску 0.2.0 было не из чего собрать.

## Почему не `msvc` с Mac

`cargo check --target x86_64-pc-windows-msvc` на Mac работает без линкера — им проверяется clippy под Windows.
Для сборки `.exe` под `msvc` нужны CRT и Windows SDK от Microsoft: `cargo-xwin` скачивает их, но только после принятия лицензии Microsoft.
Принимать лицензию за владельца агент не вправе, а сам вопрос владельцу ничего не сказал.

## Как собирается

`brew install mingw-w64` — открытый тулчейн, без лицензионного согласия.
`rustup target add x86_64-pc-windows-gnu` и `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc`.
Сборка обоих бинарей — около 20 секунд; C-кода в зависимостях нет, `sysinfo` и `windows-sys` собираются как есть.

## Что проверено

`x86_64-w64-mingw32-objdump -p` на обоих `.exe`: только системные библиотеки Windows — `KERNEL32`, `advapi32`, `WS2_32`, `USERENV`, `ntdll`, `pdh`, `psapi`, `powrprof`, `shell32`, `oleaut32`, `bcryptprimitives` и `api-ms-win-crt-*` (UCRT, есть в Windows 10 и новее).
`libgcc` и `winpthread` не нужны: Rust собирает их статически.
Запуск на самом Windows — живая проверка на GPD.

`releaseBuild.sh` собирает Windows-архив рядом с macOS и Linux, `releaseBuild.ps1` удалён, страница «Скачать» знает платформу `x86_64-pc-windows-gnu`.
Clippy под Windows по-прежнему гоняется на цели `msvc`: ему линкер не нужен, и это та цель, что у Claude Code на Windows.
