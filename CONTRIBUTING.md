# Cómo contribuir a Nebula Mail

¡Gracias por ayudar! Esta guía es corta; antes de un cambio grande, abre un issue para hablarlo.

_English summary at the end._

## Antes de empezar

- **Fallos de seguridad: nunca en un issue público.** Sigue [SECURITY.md](SECURITY.md) (Security Advisory privado de GitHub o security@helixnebula.tech).
- Para cambios grandes, abre antes un issue o una discusión: así no trabajas en algo que no encaja con el plan.
- Los fallos de Stalwart van a Stalwart ([support.stalw.art](https://support.stalw.art)), no aquí. Stalwart no acepta issues, PRs ni reportes generados con IA.

## Preparar el entorno

- TypeScript: [Bun](https://bun.sh) (nunca npm ni pnpm). `bun install --frozen-lockfile`.
- Rust (el bridge): la toolchain de `bridge/rust-toolchain.toml`.
- Stalwart de pruebas: `docker/README.md`.

Antes de pedir que se revise tu cambio, todo esto debe pasar:

```sh
bun run check        # lint y formato (Ultracite: oxlint + oxfmt); bun run fix arregla lo automático
bun run check-types  # tipos con tsrs
bun run test
cd bridge && cargo clippy --all-targets -- -D warnings && cargo test
```

### La CI en local

La CI de GitHub (`.github/workflows/ci.yml`) también se puede ejecutar entera en tu máquina con `scripts/ci-local.sh`: los mismos trabajos y comandos, uno detrás de otro, con un resumen PASS/FAIL al final (y código de salida distinto de 0 si algo falla). Cada trabajo guarda su log en `.ci-local/<fecha>/<trabajo>.log` (ignorado por git).

```sh
scripts/ci-local.sh                      # todo (= all)
scripts/ci-local.sh web rust             # solo esos trabajos
FUZZ_SECONDS=10 scripts/ci-local.sh fuzz # fuzzing más corto (por defecto 60 s por objetivo, como la CI)
CI_LOCAL_SKIP_MAIL=1 scripts/ci-local.sh bridge-servers  # sin enviar correo por Stalwart
```

Trabajos: `web`, `hostile-emails`, `web-browser`, `static-stalwart`, `rust`, `fuzz`, `bridge-servers`, `cargo-deny`, `reuse`. `bridge-servers` usa los contenedores de desarrollo que ya tengas (Stalwart y Dovecot, `docker/README.md`); no los recrea ni toca `docker/.env`. `static-stalwart` crea su propio Stalwart desechable (puerto 8095) y lo borra al terminar. Stalwart limita los envíos de alice a bob (unos 25 por hora): no lo lances en bucle. Las opciones están al principio del script. Si cambias `ci.yml`, cambia también el script (y al revés).

Reglas que la revisión comprueba: nada de `unsafe` ni `unwrap` en Rust; todo lo que descargue URLs ajenas pasa por `net-guard` y por `security/ssrf-vectors.json` (§8); la app solo importa UI de `@nebulamail/ui`; dependencias nuevas, las mínimas y justificadas (§9); cabeceras SPDX en los archivos nuevos (REUSE).

### Releases

Una etiqueta `v<versión>` lanza `.github/workflows/release.yml`: zip estático, bridge para x86_64 y aarch64 (musl, estático), SBOM, `SHA256SUMS`, firmas de Sigstore sin claves y procedencia SLSA, y deja un borrador de release para publicarlo a mano. Antes de etiquetar, sube la versión en `apps/web/package.json` y en `[workspace.package]` de `bridge/Cargo.toml` (el workflow falla si no coinciden con la etiqueta). `scripts/release/dry-run.sh <versión>` hace los mismos pasos de compilación en tu máquina (Bun y Docker), sin firmar. Cómo comprobar una release: `SECURITY.md`.

## Licencia de lo que aportas

Lo que envíes queda con la licencia del proyecto (AGPL-3.0-only, o MIT OR Apache-2.0 en las librerías), como dicen los [términos de GitHub](https://docs.github.com/en/site-policy/github-terms/github-terms-of-service#6-contributions-under-repository-license) (D.6). Tu copyright sigue siendo tuyo. No hace falta firmar los commits ni ningún acuerdo.

## Contribuciones hechas con IA

Este proyecto se desarrolla también con asistentes de IA, así que puedes usarlos. Las condiciones:

- **Tú respondes de todo lo que envías**: lo has leído, lo entiendes, lo has probado y puedes explicarlo en la revisión, igual que con el código escrito a mano.
- Dilo en la pull request (p. ej. «escrito con ayuda de Claude» o una línea `Co-Authored-By:` en el commit). No penaliza; ayuda a revisar.
- Nada de pull requests masivas generadas sin revisar, ni cambios de formato o «mejoras» automáticas sin un motivo concreto.
- No pegues código de terceros que el asistente haya podido reproducir de memoria si no puedes comprobar su licencia.
- Los **reportes de seguridad** hechos con IA tienen que estar reproducidos y comprobados por una persona (SECURITY.md).
- Nunca envíes a Stalwart issues, PRs ni reportes generados con IA: no los aceptan.

## Pull requests

- Una cosa por pull request, con una descripción que diga qué cambia y por qué (y cómo lo has probado).
- Si cambias una decisión del proyecto o tomas una nueva, explícala en la descripción.
- Mensajes de commit en español o en inglés, en imperativo y con una primera línea corta.
- Respondemos lo antes posible; si en dos semanas no has tenido noticias, escribe un comentario.

---

## English summary

- **Security issues: never in a public issue**; see [SECURITY.md](SECURITY.md).
- Open an issue before large changes. Stalwart's own bugs go to [support.stalw.art](https://support.stalw.art).
- Tooling: Bun only (no npm/pnpm). Before asking for review: `bun run check`, `bun run check-types`, `bun run test`, and for the bridge `cargo clippy --all-targets -- -D warnings` and `cargo test`. `scripts/ci-local.sh` runs the whole CI locally (all jobs or some, logs in `.ci-local/`).
- **License:** contributions are under the project's license (inbound = outbound, GitHub ToS D.6); you keep your copyright. No sign-off or CLA needed.
- **AI-assisted contributions are welcome**, but you are responsible for every line: you have read, understood and tested it. Say so in the pull request. No bulk, unreviewed AI pull requests. AI-assisted security reports must be reproduced and checked by a person. Never send AI-generated issues, PRs or reports to Stalwart.
