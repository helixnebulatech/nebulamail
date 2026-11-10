# calcard 0.4.0 y jmap-tools 0.1.9 (Stalwart), copiados

Copia de `crates/calcard` y `crates/jmap-tools` del monorepo de Stalwart
(<https://github.com/stalwartlabs/stalwart>), etiqueta **`v1.0.0`**, commit
`47df4c0baeb88d83424dfda6aa7cf6040706999e` (2026-10-26). Se usan como
dependencias por ruta (`calcard` en `[workspace.dependencies]` de
`bridge/Cargo.toml`; `jmap-tools` en `../jmap-tools`, como en el monorepo).

**Por qué ese commit:** calcard 0.4 entró en el monorepo justo en `v1.0.0`
(el repo propio, `stalwartlabs/calcard`, se cerró ese mismo día: «Development
moved to Stalwart repository»). Stalwart 1.x usa la misma calcard 0.4, así
que la prueba diferencial compara el mismo código en los dos.

## Licencia

Los dos crates son `AGPL-3.0-only OR LicenseRef-SEL` (cabeceras SPDX de cada
`.rs`, sin tocar). **Los usamos bajo AGPL-3.0-only**, la licencia del
proyecto; la SEL (licencia comercial de Stalwart) no se aplica. Se guardan los
dos textos en `LICENSES/` de cada carpeta. Consecuencias:

- Solo pueden depender de ellos crates AGPL: `deny.toml` lo impone con
  `wrappers` (hoy solo `nebula-groupware`; `jmap-tools` solo desde
  `calcard`). Las librerías MIT/Apache (`dav-proto`, `jmap-proto`…) nunca.
- REUSE: anotación en `REUSE.toml` con `precedence = "override"` para
  `bridge/vendor/calcard/**` y `bridge/vendor/jmap-tools/**`, como lettre.

## Cambios

Cada cambio en el código lleva un comentario `PATCH (nebulamail, PATCHES.md)`.
`Cargo.toml.orig` es el `Cargo.toml` original, para comparar.

1. **mail-parser con `full_encoding`** (`Cargo.toml`). El original pide
   `mail-parser = "1.0"` sin features; aquí lleva `full_encoding`, como el
   resto del workspace, para que los fuzzers de `crates/groupware/fuzz` (donde
   calcard va sola) decodifiquen `CHARSET=` de vCard 2.1/3.0 igual que el
   bridge. Hasta el 2026-10-30 había además un cambio en
   `src/common/decode.rs` para usar mail-parser 0.11.9; se quitó al pasar el
   bridge a la 1.0 y ese archivo vuelve a ser el original.
2. **Lints** (`Cargo.toml` de los dos): `[lints] workspace = true` apuntaba a
   los `[workspace.lints]` de Stalwart, que aquí no existen; se copian (son
   dos: `unexpected_cfgs` con `cfg(stalwart_tests)` y
   `clippy::result_unit_err = "allow"`).
3. **Nombres comprobados al escribir** (`src/common/writer.rs`,
   `src/vcard/writer.rs`, `src/vcard/rkyv_writer.rs`,
   `src/icalendar/writer.rs`, `src/icalendar/rkyv_writer.rs`). calcard
   escapa los valores, pero escribía tal cual los nombres de componente
   (`BEGIN:`/`END:`), grupo, propiedad y parámetro. Esos nombres pueden
   venir de JSON (`vCard.properties`, `convertedProperties`,
   `iCalendar.properties`, `iCalendar.components`), y uno con CRLF metía
   líneas nuevas: otra `VCARD` u otro `VEVENT` en el mismo PUT. Ahora
   `FoldingWriter::write_name` devuelve `fmt::Error` si el nombre tiene un
   carácter de control, `:`, `;` o `"`. No se aplica la gramática completa
   (`[A-Za-z0-9-]`): el parser acepta nombres como `X_FOO` de otros
   clientes y esas tarjetas se tienen que poder seguir editando.
   `nebula-groupware` convierte ese error en `ConvertError::Export`
   (`write_text`, nunca `to_string()`, que entraría en pánico).
4. **`CalendarExpand.truncated`** (`src/icalendar/dates.rs`). `expand_dates`
   resta 1 a `limit` por cada fecha de una RRULE, también las que quita un
   `EXDATE` o sustituye un `RDATE`, así que contar `events` no dice si la
   expansión se cortó: una serie sin fin con un `EXDATE` parecía completa y
   se indexaba con fin. El campo nuevo vale `true` cuando una regla acaba
   con `limit` agotado (o ni empieza por eso): "quizá hay más".
5. **`CHARSET=UTF-8` solo en 2.1** (`src/vcard/writer.rs`, 2026-11-06). El
   original lo ponía en 2.1 y 3.0 cuando un valor no es ASCII; RFC 2426
   (3.0) no define ese parámetro (el juego de caracteres es el del
   transporte: UTF-8 en CardDAV, RFC 6352 §5.1) y Apple no lo escribe.
   (vCard 3.0 según RFC 2426).
6. **Duraciones válidas en iCalendar** (`src/common/format.rs`,
   `src/jscalendar/import/convert.rs`, 2026-11-08). RFC 8984 §1.4.6 admite
   semanas con días u horas (`P1W2D`, `P1WT2H`); RFC 5545 §3.3.6 no
   (`dur-value = "P" (dur-date / dur-time / dur-week)`), y calcard las
   escribía tal cual en `DURATION`, `ESTIMATED-DURATION`, `TRIGGER`…
   `push_duration` las escribe ahora en días (`P9D`, `P7DT2H`): semanas y
   días son nominales, así que es exacto; una semana sola sigue como
   `P2W`. Vale también para la salida JSCalendar (válida igual). Al leer,
   un `DURATION` o `ESTIMATED-DURATION` negativo (RFC 5545 §3.8.2.5 y RFC
   10051 §10.1 piden una positiva; la Duration de JSCalendar no tiene
   signo) no se convierte: queda en `iCalendar.properties` y vuelve igual.
7. **`estimatedDuration` solo en `VTODO`**
   (`src/jscalendar/export/convert.rs`, 2026-11-08). Es de `Task` (RFC 8984
   §5.2.3) y `ESTIMATED-DURATION` solo va en `VTODO` (RFC 10051 §10.1);
   calcard lo escribía también en un `VEVENT`. En un `Event` sale como
   `JSPROP` (draft-ietf-calext-jscalendar-icalendar §4.1.2: «Other
   properties MAY be converted to JSPROP») y vuelve igual. No se rechaza:
   RFC 8984 no lo prohíbe en un Event y así no se pierde.
8. **Lo que no se copia:** nada más del monorepo. Los crates no traen
   `README`. Sus tests no están en el crate: viven en un repo privado de
   Stalwart (`stalwart-tests`, módulos `#[cfg(all(test, stalwart_tests))]`
   con `#[path = "../../../../../stalwart-tests/..."]`), así que no se pueden
   correr; esos módulos quedan desactivados (nunca se define
   `stalwart_tests`). Las pruebas propias están en `crates/groupware`
   (`tests/convert.rs` y los fuzzers de `fuzz/`).

Features que usamos: `jmap` (JSCalendar/JSContact) y `tzdb-bundle` (la base
de datos de zonas horarias va dentro del binario, `jiff/tzdb-bundle-always`:
mismo resultado en cualquier sistema). Sin `rkyv`.

## Seguridad

- Sin `unsafe`: los dos tienen `#![forbid(unsafe_code)]` en `src/lib.rs` y
  no hay ningún bloque `unsafe` (`grep`). Sí lo usan por dentro algunas
  dependencias: `ahash`, `smallvec` (feature `union`), `simdutf8`, `memchr`,
  `zerocopy` (de ahash) y `jiff`; todas muy usadas.
- calcard pone sus propios topes: profundidad de componentes (32),
  observancias de zona (4096), `MAX_EXPANSION_WORK` y presupuesto de trabajo
  improductivo en la expansión (una RRULE que nunca casa termina). Encima,
  `nebula-groupware` limita el tamaño de entrada y llama a calcard dentro
  de `catch_unwind`.
- Fuzzing: ver `crates/groupware/fuzz`.

## Actualizar

1. `git clone --filter=blob:none --no-checkout --depth 1 --branch <tag>
   https://github.com/stalwartlabs/stalwart` y
   `git sparse-checkout set --no-cone /crates/calcard/ /crates/jmap-tools/ /LICENSES/`.
2. Copiar `src/` y `Cargo.toml` (también como `Cargo.toml.orig`) de los dos
   crates y volver a aplicar los cambios de arriba.
3. Comprobar que las dependencias nuevas tienen 2 días
   (`scripts/check-release-age.py`), `cargo deny check`, `cargo test -p
   nebula-groupware` y los fuzzers.
