# MEYAFORGE

<p align="center">
  <img src="ui/assets/title.jpg" alt="MEYAFORGE" width="600">
</p>

**MEYAFORGE** — десктопная «кухня» для работы с Android-прошивками: разбор, редактирование и сборка образов в удобном графическом интерфейсе. Собрана на **Tauri** (Rust + веб-UI).

## Возможности

- Работа с образами: `sparse`, `ext4`, `super` (lpmake/lpunpack), `sdat` (dat → img)
- Сборка и пересборка прошивок (boot, vendor и др.)
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
