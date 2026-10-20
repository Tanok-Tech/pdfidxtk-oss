# Instalación de `pitk` en otras máquinas

`pitk` es un binario Rust. Hay tres formas de instalarlo; elige según la máquina del auditor.

## Requisitos por sistema

| Sistema | Para compilar desde fuente | Para usar binario ya compilado |
|---|---|---|
| macOS (Apple Silicon o Intel) | `rustup` + Xcode Command Line Tools (`xcode-select --install`) | nada |
| Windows 10/11 | `rustup` + **Visual Studio Build Tools** (C++ workload) | nada |
| Linux | `rustup` + `build-essential` | nada |

Rust se instala con:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # macOS / Linux
```

Windows: descargar `rustup-init.exe` de <https://rustup.rs> y aceptar las opciones por defecto.

---

## Opción A — Binario ya compilado (recomendada para auditores)

Descarga el binario del release (público):

```bash
gh release download --repo Tanok-Tech/pdfidxtk-oss --pattern "pitk-macos-arm64" -O pitk
chmod +x pitk

# opcional: dejarlo en el PATH
mkdir -p ~/.local/bin && mv pitk ~/.local/bin/
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc

pitk --version
```

Si macOS lo bloquea por cuarentena:

```bash
xattr -d com.apple.quarantine ~/.local/bin/pitk
```

> Windows: descargar `pitk-windows-x86_64.exe`, renombrarlo a `pitk.exe` y copiarlo a una
> carpeta del PATH (p. ej. `%USERPROFILE%\.local\bin`).

---

## Opción B — Instalar desde GitHub con Cargo

Requiere Rust instalado.

```bash
cargo install --git https://github.com/Tanok-Tech/pdfidxtk-oss.git --locked
```

Queda en `~/.cargo/bin/pitk`. Si no está en el PATH:

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc
```

Actualizar más adelante:

```bash
cargo install --git https://github.com/Tanok-Tech/pdfidxtk-oss.git --locked --force
```

---

## Opción C — Copiar el código fuente e instalarlo

Si te pasan la carpeta del proyecto (o la clonas):

```bash
cd pdfidxtk-oss
cargo install --path . --locked
pitk --version
```

Sin instalación global (útil para probar):

```bash
cargo build --release
./target/release/pitk --version
```

Ventaja: queda la versión exacta del código copiado. Desventaja: cada máquina necesita
toolchain de Rust y compila (~1 min).

---

## Verificación en cualquier máquina

```bash
pitk --version          # pitk 0.5.0
pitk init --help
```

Prueba rápida con una carpeta de PDFs:

```bash
mkdir -p ~/prueba-pitk && cd ~/prueba-pitk
cp /ruta/a/un.pdf .
pitk init --account PRUEBA
pitk index
pitk info
```

Si `pitk index` responde y `pitk info` lista el documento, la instalación está bien.
`tesseract` solo hace falta para OCR de páginas escaneadas:

```bash
brew install tesseract tesseract-lang      # macOS
```

---

## Actualizar / desinstalar

| Acción | Opción A | Opción B / C |
|---|---|---|
| Actualizar | descargar de nuevo el release | `cargo install ... --force` |
| Desinstalar | borrar el binario | `cargo uninstall pdfidxtk` |

---

## Notas

- El repo **no** contiene PDFs, datos personales ni stores `.docidx`: solo código y
  documentación. La evidencia se queda siempre en la carpeta de cada caso.
- El binario no contiene datos: se puede copiar por cualquier canal interno.
