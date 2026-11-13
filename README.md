# MEYAFORGE

<p align="center">
  <img src="ui/assets/title.jpg" alt="MEYAFORGE" width="600">
</p>

**MEYAFORGE** — десктопная «кухня» для работы с Android-прошивками: разбор, редактирование и сборка образов в удобном графическом интерфейсе. Собрана на **Tauri** (Rust + веб-UI).

## Возможности

- Работа с образами: `sparse`, `ext4`, `super` (lpmake/lpunpack), `sdat` (dat → img)
- Сборка и пересборка прошивок (boot, vendor и др.)
- **Патчинг Boot & Ramdisk**: SELinux Permissive, отключение DM-Verity & AVB, отключение ForceEncrypt, ADB root по умолчанию, ручной редактор `cmdline`, HEX-патчер ядра (`skip_initramfs`, `enforcing`)
- **LK / Bootloader & OEM команды**: извлечение скрытых Fastboot OEM команд, карты разделов, SoC-сигнатур и флагов безопасности из `lk.bin`, `abl`, `xbl`
- **ELF & Binary патчер**: разбор символов/функций в `.so` библиотеках и бинарниках, фильтр проверок безопасности (root/tamper/signature) и установка архитектурно-зависимых заглушек (`Return 0`, `Return 1`, `Return -1`, `Void`, `NOP`, `Custom HEX`), замена ASCII-строк
- **VBMeta & AVB 2.0 / 3.0**: анализ заголовков AVB0, отключение verity & verification в образе, генератор чистых пустых `vbmeta.img`
- **Device Tree Studio (DTB / DTBO & DTS)**: парсер FDT (0xD00DFEED), извлечение совместимых плат/SoC, декомпиляция в `.dts` и обратная компиляция
- **Root Инжектор**: автоматическое внедрение хуков и структуры `overlay.d` для Magisk, KernelSU и APatch в `boot.img`
- **Каталог твиков `build.prop`**: Pixel 8 Pro спуфинг (Google Photos, 90/120 FPS), оптимизация автономности и сна, аппаратное ускорение UI, отключение логирования
- **Деблоатер прошивок (Debloat Studio)**: пакетное сканирование и удаление мусора/рекламы (Xiaomi MSA/Analytics, Samsung Bixby/GOS, Google bloat, трекеры) с безопасным бэкапом в `removed_bloat/`
- **Fastboot Flasher & Режимы**: прошивка разделов по слотам (`_a` / `_b`), перезагрузка в Bootloader, Recovery, FastbootD и аварийный EDL (Qualcomm 9008)
- Портирование прошивок между устройствами
- Чтение разделов с устройства (device reader, ADB-инструменты)
- Встроенный файловый менеджер для содержимого образов
- Интеграция с Telegram (отправка готовых сборок)

## Стек

- **Backend:** Rust (Tauri) — `src-tauri/src`
- **Frontend:** HTML/CSS/JS — `ui/`

## Сборка и запуск

Требования: [Rust](https://rustup.rs/), Node.js не обязателен (UI статический).

```bash
# режим разработки
cargo tauri dev

# сборка релизного приложения
cargo tauri build
```

Команды выполняются из корня репозитория (конфиг Tauri — `src-tauri/tauri.conf.json`).

## Структура проекта

```
├── src-tauri/      # Rust-бэкенд: работа с образами, сборка, инструменты
│   └── src/
└── ui/             # Интерфейс приложения
    └── assets/     # Иконки и изображения
```
