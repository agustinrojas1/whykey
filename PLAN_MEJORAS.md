# Plan de mejoras de Whykey

## Evaluación

La base es aprovechable: Rust con pocas dependencias directas, resultados de ruta tipados, límites de tiempo y salida para comandos externos, adaptadores separados, compatibilidad de schemas y pruebas de CLI/PTY. No propongo reescribir el programa.

Antes de ampliar integraciones, hay que corregir la precisión de los keycodes, endurecer la captura y hacer que la conclusión respete la evidencia. La salida actual reúne datos útiles, pero mezcla observación real, configuración e inferencias en una sola evaluación global.

Alcance de esta revisión: captura Hyprland/terminal, XKB, IME, construcción de la ruta, reportes, contratos de salida y comprobaciones generales. No es una auditoría exhaustiva de todos los adaptadores.

## Verificación realizada

Sobre `rebuilt-main`, commit `f55ff6f`, inicialmente limpio:

- `cargo test --all-targets --locked`: 439 tests pasaron; 3 de latencia/throughput ignorados.
- `cargo clippy --all-targets --locked -- -D warnings`: pasó.
- `cargo fmt --all -- --check`: pasó.
- Comprobaciones de matriz de soporte, procedencia de dependencias y versiones de paquetes: pasaron.
- `cargo run --locked -- --help`: funcionó.
- Reproducción histórica del renderer (antes de `BindingEvidence`): `HandledUncertain` en una captura suprimida generaba `would handle and consume` sin detalle reconocido, o `would run Herdr` con la descripción. Ahora la conclusión lee evidencia tipada y un dispatcher opaco nunca se presenta como ejecutado.
- Reproducción XKB histórica (antes de los tipos `EvdevKeycode`/`XkbKeycode`): `symbols_for_evdev_keycode(..., 28, 0)` devolvía `Return, T, t` por consultar ambos namespaces; ahora sólo XKB 36 devuelve `Return`.

Estado actual: las pruebas con Hyprland real viven en `tests/hyprland_live.rs`, marcadas `#[ignore]` y condicionadas a `WHYKEY_RUN_LIVE_TESTS=1` más `WHYKEY_LIVE_INSTANCE_SIGNATURE` igual a la firma activa. La suite común ya no contacta ni modifica el compositor; fuera de una sesión dedicada, las live fallan con mensaje de prerrequisito en vez de pasar en silencio.

No se ejecutó una matriz completa de versiones de Hyprland, ni se verificó MSRV en esta revisión. Los fallos de activación y recuperación descritos abajo son rutas identificadas por lectura de código, no incidentes provocados en el escritorio.

## Qué muestra el ejemplo CTRL + SUPER + RETURN

- La observación de XKB 36 / evdev 28 es coherente con Enter en el esquema evdev de XKB.
- `Return, T, THORN, t, thorn` no debe presentarse como símbolos de una única tecla física. La función consulta simultáneamente el código evdev crudo y el desplazado para XKB.
- Encontrar un binding etiquetado `Herdr` no prueba qué ejecuta el dispatcher opaco `__lua 285` ni si reenviaría el evento.
- `conditional` es razonable para esa propagación no observable. No significa que la captura haya fallado.
- Un proceso Fcitx5 detectado, un engine seleccionado y composición activa son datos distintos. El estado `inactive` debe quedar explícito, sin afirmar tampoco que descarta todo Compose/preedit del lado de la aplicación.
- La supresión temporal y el comportamiento de la configuración normal son dos escenarios diferentes. El informe debe separarlos.

## Fase 0. Aislar las pruebas que alteran el escritorio

Prioridad: bloqueante para continuar verificando con seguridad.

Archivos: `src/hyprland_capture.rs:1442`, `src/hyprland_capture.rs:1498`, tests de integración y CI.

1. Sacar las pruebas con `dispatch`, captura real y `reload` del recorrido común. Exigir habilitación explícita y una sesión de pruebas dedicada.
2. Sustituir IPC real por dobles controlados en tests unitarios y de integración comunes.
3. No contabilizar un retorno temprano por falta de entorno como validación del comportamiento live. Informar que no se ejecutó.
4. Documentar qué prueba modifica qué estado y cómo se recupera.

Aceptación (cumplida en 1.0.1):

- La suite estándar no contacta ni modifica el compositor real: las pruebas live están en `tests/hyprland_live.rs` con triple opt-in.
- Funciona sin Hyprland instalado.
- Las pruebas live requieren opt-in y comprueban restauración del submap original registrado, no sólo ausencia de `__whykey_capture`.

## Fase 1. Corregir precisión y certezas falsas

Prioridad: alta. Dos frentes independientes, con integración al terminar.

### 1A. Unificar el significado de los keycodes

Archivos: `src/xkb.rs:102`, `src/xkb.rs:141`, `src/layers/hyprland.rs:1644`.

- Introducir tipos distintos para evdev y XKB y convertir una sola vez en el límite de entrada.
- Eliminar la unión heurística de `code` y `code + 8` cuando ya se conoce el namespace.
- Verificar el contrato del campo de bindings en las versiones soportadas. Si alguna variante usa otro namespace, representarla explícitamente.
- Revisar todos los consumidores de las funciones afectadas, incluido el símbolo preferido y matching numérico.
- Evitar compilar silenciosamente un layout US como si fuera el activo cuando no se obtuvo RMLVO. La ruta Hyprland usa defaults en `src/layers/hyprland.rs:1144`; cualquier fallback debe quedar identificado como supuesto y no aportar certeza de matching.

Aceptación:

- En keymap US, evdev 28 produce Return, nunca T/t.
- Una captura evdev 28 no coincide con un binding XKB 28 por igualdad numérica accidental.
- Cubrir también casos positivos, Shift, AltGr, NumLock, grupos y datos RMLVO faltantes.
- Layout o dispositivo desconocidos producen una limitación explícita, no candidatos inventados.

### 1B. La conclusión no debe ser más fuerte que el resultado

Archivos: `src/report.rs:431`, `src/report.rs:451`, `src/report.rs:617`, `src/layers/mod.rs:198`, `src/schema.rs:34`.

- No convertir `HandledUncertain` en consumo ni describir una etiqueta Lua como una ejecución verificada.
- Separar certeza de captura, coincidencia de binding, acción esperada y propagación.
- Empezar a representar binding, dispatcher, descripción, submap, alcance y origen como datos tipados. El renderer hoy recupera semántica parseando `details` y buscando textos como `(all submaps)`.
- Mantener compatibilidad de schema v1/v2. Si hay cambios incompatibles, versionarlos explícitamente; no cambiar silenciosamente el significado de campos existentes.

Aceptación:

- Regresión para `HandledUncertain` con y sin descripción: nunca afirmar consumo.
- Cambiar el texto de un detalle no cambia la decisión sobre supresión o propagación.
- Dispatchers opacos y bindings universales mantienen sus límites tanto en texto como en JSON/NDJSON.

## Fase 2. Endurecer el ciclo de captura

Prioridad: alta. Puede investigarse en paralelo con 1A, después de aislar los tests.

Archivos: `src/hyprland_capture.rs:157`, `src/hyprland_capture.rs:205`, `src/hyprland_capture.rs:455`, `src/listen.rs:492`.

Riesgos identificados por código:

- El armado emite `armed` después de invocar dispatch, sin validar su resultado como hace otra ruta de cleanup. Un resultado de error que no lance excepción podría quedar tratado como éxito.
- El cleanup marca la guardia como no instalada antes de completar la operación, y descarta el resultado del fallback.
- La restauración previa al reporte descarta el error.
- El failsafe Lua descarta el resultado de dispatch al limpiar. Ejecutar un intento no prueba haber restaurado.

Cambios:

1. Validar resultado y postcondición antes de anunciar captura armada.
2. Modelar estados de instalación, armado, restauración pendiente y cierre confirmado.
3. Mantener recuperación pendiente y contexto original cuando falla la restauración. Aplicar reintentos acotados, respetando ownership; nunca pisar otra sesión.
4. No ocultar errores de restauración al renderizar. Si se conserva el diagnóstico capturado, acompañarlo de fallo operacional explícito y salida no exitosa.
5. Diferenciar submap original desconocido de `default` confirmado.
6. Mantener explícita la excepción de bindings universales y el alcance limitado de la supresión.

Aceptación con IPC/Lua simulados:

- Dispatch devuelve `false`, tabla de error o éxito sin cambio de submap: no se anuncia supresión confirmada.
- Fallo de restauración, fallback fallido y segundo intento exitoso.
- Token viejo no puede desarmar una captura nueva.
- Pérdida de estado Lua durante reload, submap inicial personalizado y consulta inicial fallida.
- Señal durante armado, captura y cleanup; caída del socket; cancelación y timeout.
- Failsafe conserva una vía de recuperación si el dispatch falla.

Luego, en sesión live dedicada: verificar SIGKILL del cliente, restauración por timer, reload real, dos listeners y bindings universales. No prometer garantías que dependen de que el compositor siga vivo y responda.

## Fase 3. Hacer legible el diagnóstico

Prioridad: media, después de fijar el significado de los resultados.

Archivos: `src/report.rs:287`, `src/report.rs:418`, `src/ime.rs:132`, `src/layers/mod.rs:459`.

- Poner el resultado antes de Capture. Hoy se imprime el bloque técnico completo primero.
- Una sola frase de captura/supresión, sin repetición.
- Vista normal con binding relevante, origen y motivos concretos de incertidumbre. Raw event, inventario de teclados, niveles XKB y submaps inactivos en `--verbose`.
- Tratar IME como contexto cuando no hay evidencia de intervención. No añadirlo como un salto efectivamente recorrido si el análisis terminó antes de esa capa.
- Conservar la advertencia sobre Compose/preedit cuando sea pertinente; no inferir ausencia de transformación únicamente de `inactive`.
- Explicar por qué falta certeza y qué observación permitiría mejorarla. No recomendar ejecutar automáticamente dispatchers desconocidos.

Aceptación:

- El caso del usuario se entiende en una pantalla sin perder los límites importantes.
- Tests de salida normal/verbose y consistencia con JSON.
- Fcitx5 activo, inactivo, cerrado e inaccesible tienen mensajes diferenciados.

Ejemplo de contenido deseado, no salida actual ni garantía live validada:

- Tecla capturada: CTRL + SUPER + RETURN.
- Binding en la configuración normal: Hyprland, descripción Herdr, submap default.
- Dispatcher: Lua opaco; no ejecutado por Whykey.
- Propagación normal: no verificable.
- Estado de supresión: mostrar sólo lo comprobado y sus excepciones.
- Origen: archivo y línea, como pista estática cuando no hay procedencia runtime demostrada.

## Fase 4. Agregar funciones que ayuden a resolver casos

Prioridad: posterior a precisión y seguridad.

1. **Snapshot reproducible de entradas de diagnóstico.** Ya existe `replay`, pero vuelve a representar reportes guardados. Agregar un paquete opt-in con evidencias necesarias para repetir el análisis offline, versión del programa y contexto. Minimizar datos y redactar rutas privadas/comandos sensibles antes de compartir.
2. **Comparación de snapshots.** Mostrar qué binding, layout o contexto cambió entre un estado que funcionaba y otro que falla.
3. **Doctor con próximos pasos específicos.** Distinguir integración no instalada, permiso faltante, IPC inaccesible y dato que la API no expone. Ofrecer comprobaciones read-only, no reparaciones automáticas del escritorio.
4. **Filtros para bindings y conflictos.** Por tecla, acción, dispositivo y submap, preservando la diferencia entre duplicado textual, coincidencia potencial y conflicto de activación demostrado.

No reinventar funciones existentes: `doctor`, `bindings`, `conflicts`, `replay`, `--ndjson`, `--output`, `--focused`, `--pid` e integración con shells ya están implementadas.

## Mantenimiento y documentación

- Tras las regresiones, separar parsing, transporte y ciclo de captura en los archivos grandes. No hacer una reorganización masiva junto con fixes semánticos.
- Corregir la promesa global de read-only en README/ayuda: inspección no ejecuta shortcuts ni edita archivos de configuración; `listen` nativo sí modifica temporalmente el estado de la sesión.
- Reconciliar `DEFECT_REGISTER.md`: varios elementos siguen abiertos y descritos como blockers aunque citan fixes/regresiones. Cerrarlos sólo con evidencia y dejar visibles los límites aceptados.
- Conservar pruebas de contrato para soporte y schemas; añadir pruebas negativas de matching y fallos de IPC, no sólo comprobaciones de substrings Lua.

## Orden de entrega

1. Tests aislados y live opt-in.
2. Keycodes y certeza del renderer, con regresiones.
3. Captura verificable y recuperación explícita.
4. Salida breve, evidencia tipada e IME contextual.
5. Snapshots/diff y diagnósticos accionables.
6. Refactor selectivo y ampliación de adaptadores sólo donde haya casos verificables.

No agregaría ahora una GUI, ejecución automática de shortcuts, privilegios elevados por defecto ni más adaptadores nominales. El valor de Whykey depende primero de acertar y de no dejar el teclado en un estado inesperado.

## Verificación por fase (1.0.1)

Cada fase cierra con su commit y este gate, sin contar retornos tempranos
por falta de entorno como validación live:

- Fase 0 (tests live aislados): `test(hyprland): isolate compositor-mutating tests`. `cargo test --all-targets --locked` nunca muta Hyprland; `cargo test --test hyprland_live -- --ignored` falla con mensaje claro fuera de una sesión dedicada (`WHYKEY_RUN_LIVE_TESTS=1` + `WHYKEY_LIVE_INSTANCE_SIGNATURE`).
- Fase 1A (keycodes): `fix(input): separate evdev and XKB keycodes`. Evdev 28 produce Return y nunca coincide con XKB 28; niveles Shift/AltGr/NumLock y grupos cubiertos; sin RMLVO hay incertidumbre, no layout US.
- Fase 1B + 2 (captura y certeza): `fix(hyprland): model capture and restoration states` y `refactor(report): use typed binding evidence`. `CaptureState` con reintentos acotados y `RestorePending` ante fallos; la conclusión lee `BindingEvidence` tipada y un dispatcher opaco nunca se presenta como ejecutado.
- Fase 3 (salida legible): `refactor(report): keep normal output concise`. El reporte normal cabe en una pantalla; `--verbose` conserva todo el diagnóstico; texto y JSON comparten conclusión.
- Fase 4 (filtros): `feat(filters): add device and submap filters`. `--device`/`--submap` con AND insensible a mayúsculas; sin metadatos no hay coincidencia.
- Snapshot (contrato aceptado): `docs(snapshot): define the diagnostic snapshot contract`. El snapshot guarda un reporte redactado para replay y comparación, no entradas reproducibles para reejecutar adaptadores offline. Requiere retener salidas de comandos y extractos de configuración por adaptador, más un proveedor de entradas para replay: queda programado después de 1.0.1.

Comportamiento live (captura sin abrir Herdr, Escape/Ctrl+C, repeat, reload,
SIGKILL, segundo listener, `--pass-through`) sólo se da por verificado con
la sesión dedicada: `WHYKEY_RUN_LIVE_TESTS=1 cargo test --test hyprland_live -- --ignored --nocapture`.
