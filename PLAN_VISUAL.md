# Plan de mejora visual de whykey

Objetivo: que el resultado de `whykey <combinación>` se entienda de un vistazo,
sin conocer el vocabulario interno del programa, y que la salida en terminal
refleje el mismo modelo mental que el diagrama del README: una cadena de
capas, cada una un punto de control que detiene, deja pasar o reescribe la
tecla.

Alcance: salida humana (texto/color) de `inspect`, `listen`, `doctor`,
`capabilities` y los errores de CLI. La salida JSON/NDJSON (schema v1 y v2) no
cambia: es contrato para automatización y la lab la compara.

## Diagnóstico de la salida actual

Ejemplos reales tomados en esta máquina (Hyprland, Wayland, bash) con la
versión 2.3.0.

### `whykey super+return`

```
Super+Return · Inspect

Hyprland is configured to consume the shortcut.

Effect
  The configured consumer stops this shortcut before later layers can handle it.

Relevant layers (configuration)
  Hyprland — Configured to stop: Terminal [live query]
      action __lua 268; source
      /usr/share/omarchy/default/hypr/bindings/applications.lua:2

Technical detail: use --verbose for the complete route and raw evidence.
```

Problemas:

1. **No se ve la ruta.** El programa recorre siete capas, pero sólo muestra
   la que respondió. El usuario no sabe si el remapper fue consultado, si la
   terminal quedó fuera, ni dónde se cortó la cadena. El concepto central del
   producto (la cadena) no aparece en el resultado.
2. **El veredicto está enterrado en prosa.** "is configured to consume the
   shortcut" y luego "Effect: The configured consumer stops this shortcut
   before later layers can handle it" dicen lo mismo dos veces, en jerga. Lo
   que la persona quiere leer es: *Hyprland se queda con Super+Return y abre
   la terminal; nada más la recibe*.
3. **Sin señal visual de resultado.** Los únicos colores son negrita, cian en
   los títulos de sección y atenuado. Que una capa **detenga** la tecla, la
   **deje pasar** o **no se sepa** se ve igual. En `src/style.rs` ya existen
   `failure`, `success` y `uncertain`, pero el renderer casi no los usa para
   el veredicto.
4. **Evidencia mal alineada.** `action __lua 268; source` y el path quedan en
   líneas distintas por el wrap. Un `file:line` es lo más útil del informe y
   debería ir primero, entero, en su propia línea.
5. **Cabecera poco informativa.** `· Inspect` sólo distingue de `Listen`. No
   dice en qué entorno se evaluó (compositor, sesión, shell, terminal), que es
   lo primero que uno revisa cuando el resultado no coincide con lo esperado.
6. **Ancho fijo de 80 columnas.** `style::terminal_width` lee `COLUMNS`, que
   bash no exporta por defecto, así que casi siempre envuelve a 80 aunque la
   terminal tenga 200.
7. **Vocabulario interno.** "Configured to stop", "Forwarding unknown",
   "Startup file (unverified)", "No inspected layer reports a binding;
   handling is unknown in TTY driver". Son etiquetas correctas para quien
   conoce el modelo `Outcome`, no para quien pregunta por qué no funciona su
   atajo.

### `whykey ctrl+super`

```
error: a combination must contain a non-modifier key

Try 'whykey --help'.
```

Es una pregunta legítima: Super solo abre el lanzador en GNOME/KDE, keyd y
kanata hacen *tap-hold* sobre Ctrl, Hyprland tiene `bindr` para Super suelto.
whykey ni lo explica ni lo soporta. El mensaje tampoco dice qué sí acepta.

### `whykey ctrl+left`

La conclusión ("Bash / Readline binds this to backward-word in /etc/inputrc,
but your shell config may override it") es buena, pero el resto repite la
idea tres veces (Effect, Relevant layers, Next) y de nuevo no muestra que el
compositor y el remapper fueron descartados.

### `whykey doctor`

`OK`/`FAIL` sin alinear (`OK  terminal:` lleva dos espacios), líneas de FAIL
de cinco renglones con el comando de arreglo incrustado entre paréntesis,
grupos sin título consistente y sin resumen final. `capabilities` repite el
estado tres veces por línea (`AVAILABLE cli.inspect.combination [cli;
available]`).

## Diseño propuesto

### `whykey super+return`

```
Super+Return                                        Hyprland · Wayland · bash

✕ Hyprland takes this shortcut and runs Terminal.
  Nothing after Hyprland ever receives Super+Return.

Route
  1  Remapper        ↓  none detected
  2  Hyprland        ✕  Terminal                               live query
  3  Terminal        ·  not reached
  4  TTY driver      ·  not reached
  5  Input method    ·  not reached
  6  Multiplexer     ·  not reached
  7  bash            ·  not reached

Bound in
  /usr/share/omarchy/default/hypr/bindings/applications.lua:2
  __lua 268 (Lua dispatcher; whykey does not execute it)

Confidence  live query: Hyprland reported this binding over IPC, in the
            active submap. Not observed: run `whykey listen` to watch it.

More  whykey super+return -v
```

### `whykey ctrl+left` (sin `shell-init`)

```
Ctrl+Left                                           Hyprland · Wayland · bash

? Probably bash: /etc/inputrc binds Ctrl+Left to backward-word.
  Unverified. Your .bashrc or .inputrc may override it.

Route
  1  Remapper        ↓  none detected
  2  Hyprland        ↓  no binding                             live query
  3  Terminal        ?  not inspected (not running in a terminal)
  4  TTY driver      ↓  not a control character
  5  Input method    ↓  fcitx5 present, inactive
  6  Multiplexer     ↓  none detected
  7  bash            ?  backward-word                 startup file, unverified

To confirm
  eval "$(whykey shell-init bash)"    then run whykey ctrl+left again
```

### `whykey ctrl+super`

```
error: Ctrl+Super has no main key.

  whykey inspects a combination with one main key, like `whykey super+return`
  or `whykey ctrl+shift+c`.

  To see what a modifier does on its own (Super opens a launcher, Ctrl taps
  as Esc), ask for the modifier alone: `whykey super --alone`.
```

Y, con la fase 7 implementada, `whykey super --alone` responde con la misma
tabla de ruta, consultando remappers (overload/tap-hold), GNOME
`overlay-key`, KDE (Meta → lanzador), Hyprland `bindr`/`bindrt`, sway
`bindsym --release`.

### Glifos y colores

| Glifo | ASCII | Color | Significado |
| --- | --- | --- | --- |
| `✕` | `x` | rojo | la capa se queda con la tecla; nada después la recibe |
| `↓` | `v` | verde | la capa la deja pasar (con o sin acción propia) |
| `↪` | `>` | amarillo | la capa la reescribe; sigue otra tecla |
| `?` | `?` | amarillo | no se pudo verificar: default, archivo sin cargar, dispatcher opaco |
| `·` | `-` | atenuado | no alcanzada o no aplicable |

La columna de confianza usa las cuatro palabras que ya existen en
`EvidenceTier` (`observed`, `live query`, `config file`, `default`) y nada
más. El glifo lleva el color; el texto nunca depende del color para
entenderse (`NO_COLOR`, `--color never`, `TERM=dumb` siguen funcionando).

## Fases

### Fase 0. Base de presentación

Archivos: `src/style.rs`, `src/cli.rs`.

- Ancho real: `ioctl(TIOCGWINSZ)` vía `libc` (ya es dependencia), con
  `COLUMNS` como override y 80 como último recurso. Cota superior de 120
  para la prosa.
- Paleta semántica en `RenderOptions`: `stop`, `pass`, `remap`, `unsure`,
  `off`, además de `heading`/`muted`. Un solo lugar define glifo + color.
- Glifos con fallback: si la locale no es UTF-8 (`LC_ALL`/`LC_CTYPE`/`LANG`)
  o se pasa `--ascii`, se usan los equivalentes ASCII de la tabla.
- Helper de tabla con columnas alineadas (`display_width` ya mide ANSI y
  Unicode) que nunca corte un path: si no entra, la celda pasa a su propia
  línea indentada.

Aceptación: tests de ancho con `COLUMNS` y sin TTY; tests de que con
`--color never` no hay secuencias ANSI; test de que `--ascii` no deja ningún
carácter fuera de ASCII.

### Fase 1. Veredicto y cabecera

Archivos: `src/report.rs` (`render_inner`, `diagnosis_text`, `effect_text`,
`qualification_text`).

- Cabecera: combinación a la izquierda, contexto a la derecha (compositor ·
  tipo de sesión · shell; terminal cuando se conoce). Fuente: el mismo
  `context` que ya va al JSON v2. Se elimina `· Inspect`; `· Listen` se
  reemplaza por una línea `Observed via Hyprland (pass-through)` bajo el
  veredicto, que ya existe.
- Veredicto en dos líneas: glifo + *quién* + *qué hace*; segunda línea,
  *consecuencia* (nada después la recibe / sigue hasta X / no se sabe).
  `effect_text` se funde aquí; desaparece la sección **Effect**.
- La calificación de incertidumbre (hoy en amarillo bajo el diagnóstico)
  pasa a ser la segunda línea cuando el veredicto es `?`.

Aceptación: las pruebas de `report.rs` que fijan texto (`"Ctrl+Left ·
Inspect"`, `"Super+K · Listen"`, etc.) se migran a golden files; `evaluate_conclusion`
no cambia (la lógica de conclusión queda intacta, sólo cambia cómo se
escribe).

### Fase 2. Tabla de ruta

Archivos: `src/report.rs` (`relevant_layers`, `layer_line`,
`concise_*_evidence`), `src/layers/mod.rs` (orden de `LayerId`).

- Siempre las siete capas en orden, numeradas, con nombre concreto cuando se
  conoce (`Hyprland`, `bash`, `kitty`) y genérico cuando no (`Terminal`).
- Columna de glifo según `Outcome` + `UncertaintyReason`; columna de razón
  corta (acción o motivo, máximo una línea); columna de confianza a la
  derecha.
- Capas después de una `✕` se marcan `·  not reached`. Capas `Pass` sin
  nada que decir muestran la razón más corta disponible (`no binding`,
  `none detected`, `not a control character`).
- `also_fires` se muestra como una segunda línea indentada bajo la capa
  (`also: …`), no concatenado con `;`.
- Sustituye a **Relevant layers**; en `-v` la tabla se mantiene y debajo va
  la evidencia completa.

Aceptación: golden files para los casos que ya cubren los tests
(`consumed`, `forwarded`, `redirected`, `uncertain`, `unavailable`, dead key,
`HandledUncertain`, suppression de Hyprland, universal binding); lab replays
(`cargo test` con las grabaciones en `lab/recordings`) siguen verdes.

### Fase 3. Bloque de evidencia y confianza

Archivos: `src/report.rs` (`concise_binding_evidence`,
`concise_detail_evidence`, `uncertainty_text`, `evidence_tier`).

- **Bound in**: `file:line` primero, en su propia línea; debajo
  `dispatcher/acción` y, si aplica, la nota de dispatcher opaco. Para
  readline: `sequence: ESC f` y el archivo.
- **Confidence**: una frase por tier explicando de dónde salió la respuesta
  y qué la haría más fuerte (`live query` → "not observed: run whykey
  listen"; `config file` → "file read, not confirmed loaded"; `default` →
  "program default, your config may override it"). Sustituye a **What
  remains uncertain**, que hoy es genérico ("One or more layers remain
  uncertain, so downstream behavior is conditional").

### Fase 4. Vocabulario

Archivo: `src/report.rs`; `docs/` para el glosario.

Tabla de reemplazo (texto humano únicamente; JSON conserva `status`,
`propagation`, `tier`):

| Hoy | Propuesto |
| --- | --- |
| is configured to consume the shortcut | takes this shortcut and runs *X* |
| Configured to stop | ✕ *acción* |
| Forwarding unknown | ? not sure it continues |
| Startup file (unverified) | *acción* · startup file, unverified |
| Default (unverified) | *acción* · default, unverified |
| Possible match | ? N bindings with unreported keys |
| No inspected layer reports a binding; handling is unknown in TTY driver | Nothing claims Ctrl+C before the TTY driver, which whykey could not inspect here |
| Unavailable: could not inspect terminal settings | not inspected (not running in a terminal) |
| HandledUncertain / opaque dispatcher | ? Hyprland matches it, but its Lua action is opaque |

Regla: cada frase nombra al programa concreto y a la acción concreta cuando
existen; "layer", "consumer", "downstream", "propagation" y "forwarding" no
aparecen en la salida normal.

### Fase 5. Siguientes pasos y pie

Archivo: `src/report.rs` (`next_steps`).

- Máximo dos pasos, cada uno un comando ejecutable y una razón de media
  línea. Título **To confirm** cuando el veredicto es `?`, **Next** si no.
- Pie de una línea: `More  whykey <combo> -v` (y `whykey listen` cuando
  aplica). Sustituye a "Technical detail: use --verbose for the complete
  route and raw evidence."

### Fase 6. Errores de CLI

Archivos: `src/key.rs:220`, `src/key.rs:258`, `src/cli.rs`.

- Modificador solo: explicar qué acepta whykey, con un ejemplo, y apuntar a
  `--alone` (fase 7).
- Tecla desconocida: sugerir el nombre más cercano del inventario de keysyms
  (`src/xkb_keysyms.txt`), p. ej. `enter` → `return`, `win` → `super`,
  `esc` → `escape`.
- Dos teclas principales: mostrar cómo pedir una secuencia (`whykey inspect
  ctrl+x ctrl+s`).
- Códigos de salida sin cambios.

### Fase 7. Modificadores solos

Archivos: `src/key.rs`, `src/remapper/*`, `src/layers/gnome.rs`,
`src/layers/kde.rs`, `src/layers/hyprland`, `src/layers/sway.rs`.

- `whykey super --alone` (o `whykey super` con confirmación interactiva):
  nuevo `KeyCombo` con tecla principal = el modificador y flag `alone`.
- Lectores: keyd `overload`/`timeout`, kanata `tap-hold`, kmonad
  `tap-hold`, xremap `modmap` con `held/alone`; GNOME `overlay-key`; KDE
  `kwinrc [ModifierOnlyShortcuts]`; Hyprland `bindr`/`bindrt` con la
  combinación vacía; sway `bindsym --release`.
- Mismo renderer: la tabla de ruta funciona sin cambios.

Es la fase más grande y la única que añade capacidad, no sólo
presentación. Puede ir después de las demás sin bloquearlas.

### Fase 8. `--verbose`

Archivo: `src/report.rs` (`render_verbose_evidence`).

- Agrupar la evidencia por tema con subtítulos: *binding*, *keyboard and
  layout*, *policy notes*. Hoy son ocho líneas `evidence:` seguidas.
- Colapsar la lista de teclados a `main: <nombre> (<keymap>); 7 more` con el
  detalle completo en una sola línea atenuada.
- Mantener el bloque `binding:` tipado tal cual (es lo que se copia en un
  issue).

### Fase 9. `doctor` y `capabilities`

Archivos: `src/doctor.rs`, `src/capabilities.rs`.

```
whykey doctor                                       Hyprland · Wayland · bash

Capture
  ✓  Hyprland native capture
  ✗  controlling TTY            run whykey inside a terminal
  ✗  evdev keyboards            no read access to 3 devices
       fix  sudo setfacl -m u:"$USER":r /dev/input/event2 /dev/input/event5 ...
       undo sudo setfacl -x u:"$USER" ...

Live readers
  ✓  Hyprland IPC
  ✓  gnome-gsettings
  –  portal GlobalShortcuts     registration API only, no binding inventory
  –  kglobalaccel               not running

Shell and editors
  ✓  bash                       /usr/bin/bash
  ✗  live shell bindings        eval "$(whykey shell-init bash)"
  ✓  Neovim extension           whykey-nvim installed; no server listening

Summary  7 ok · 3 unavailable · 2 not applicable
```

- Columna de estado (`✓ ✗ – ?`), nombre, razón en una línea, y `fix`/`undo`
  en líneas propias. Nunca el comando dentro de un paréntesis.
- Grupos fijos con título; `–` para lo no aplicable en esta sesión en vez de
  `FAIL`.
- `capabilities`: id, una palabra de estado, una línea de descripción; sin
  repetir el estado.

### Fase 10. `listen`

Archivos: `src/listen/output.rs`, `src/listen/mod.rs`.

- Mientras espera: una línea en stderr, `Waiting for a key… (Hyprland
  capture, pass-through; Ctrl+C cancels)`, que se borra al recibir la tecla.
- Resultado: misma tabla de ruta con la columna de confianza en `observed`
  para las capas que la captura prueba (`observation_covers`), y el veredicto
  diciendo explícitamente qué se vio y qué se infiere.
- `--dry-run` y `--explain-capture`: la cadena de captura con los mismos
  glifos (`✓ selected`, `– not needed`, `✗ unavailable`).

### Fase 11. Documentación

- `docs/assets/whykey-demo.svg` regenerado a partir de la salida real de la
  fase 2 (un script `tools/render_demo.py` que convierte la salida con
  `--color always` a SVG, para no dibujarlo a mano otra vez).
- README: sección "Reading a report" con un informe anotado (qué significa
  cada columna y cada glifo).
- `whykey.1`: la misma tabla de glifos.
- Diagrama del README: hecho en esta tanda (`docs/assets/key-path-*.svg`,
  generado por `tools/key_path_diagram.py`, con variante clara y oscura vía
  `<picture>`).

## Pruebas y compatibilidad

- **Golden files** en `tests/golden/<caso>.txt`, comparados con `--color
  never --ascii` y ancho fijo, actualizables con `WHYKEY_UPDATE_GOLDEN=1`.
  Reemplazan los `assert!(output.contains(...))` dispersos en `report.rs` y
  `tests/`. Sin dependencias nuevas.
- **JSON intacto**: un test por fase que vuelca `--json` y `--json-v2` antes
  y después y exige igualdad byte a byte sobre las grabaciones de
  `lab/recordings`.
- **Lab**: las aserciones del lab que leen texto humano (`lab/driver.py`,
  `lab/envs/*`) se revisan en la fase 2; las que leen JSON no cambian.
- **Accesibilidad**: ningún significado sólo por color; glifos con fallback
  ASCII; ancho mínimo de 40 columnas sin romper la tabla.

## Orden y tamaño

| Fase | Tamaño | Depende de |
| --- | --- | --- |
| 0 Base de presentación | S | — |
| 1 Veredicto y cabecera | M | 0 |
| 2 Tabla de ruta | M | 0, 1 |
| 3 Evidencia y confianza | S | 2 |
| 4 Vocabulario | S | 1, 2 |
| 5 Next y pie | S | 1 |
| 6 Errores de CLI | S | — |
| 8 `--verbose` | S | 2 |
| 9 `doctor`/`capabilities` | M | 0 |
| 10 `listen` | M | 2 |
| 11 Documentación | S | 2, 9 |
| 7 Modificadores solos | L | 2, 6 |

Las fases 0 a 5 son una sola entrega coherente (el informe nuevo); 6 y 9
pueden ir en paralelo; 7 es la última porque cambia el modelo de entrada y
no sólo la presentación.
