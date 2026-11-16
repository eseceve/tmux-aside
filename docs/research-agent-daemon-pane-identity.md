# Identidad de panel y estado de agentes con daemon

Investigación del 16 de noviembre de 2026. Alcance: Codex CLI 0.169.0 y Claude Code 2.1.325 instalados; fuentes públicas consultadas hoy. Incluye una validación posterior con un prototipo aislado. No se modificaron las sesiones ni la configuración instalada durante esta investigación y validación.

## Conclusión

Podemos conservar los daemons y mejorar el estado del sidebar ahora, pero no afirmar una correspondencia universal y automática entre cualquier TUI, su conversación visible y un panel. Son dos problemas diferentes: **panel → estado visible** y **panel → conversación → estado del runtime**. Codex ya emite estados desde la TUI mediante OSC 7501; Claude documenta una consulta de sesiones con identidad y estado. La captura de bytes por panel se verificó primero con señales sintéticas y luego con dos TUIs reales de Codex, un app-server compartido aislado y respuestas de modelo simuladas; la validación se describe al final. [Codex 0.169.0: emisor OSC](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/terminal_program_status.rs), [Claude: consulta de sesiones](https://code.claude.com/docs/en/agent-view#list-sessions-as-json).

Recomendación: integrar primero OSC para el indicador de Codex tras las pruebas del prototipo descritas al final, y probar `claude agents --json` para Claude; mantener una identidad explícita para lanzamientos controlados. No retirar todavía el workaround `--no-daemon` de Codex: falta integrar el observador, la precedencia de fuentes y las guardas del Watcher. La restauración exacta y el envío de instrucciones necesitan garantías adicionales a las de un indicador visual.

## Evidencia y límites

**Observaciones locales previas de esta conversación:** el daemon compartido de Codex conservaba `TMUX_PANE=%0`, mientras otras TUIs tenían su panel correcto en su propio entorno. Los hooks de tmux-aside registraban por variable de entorno. Se migraron ocho sesiones a modo embebido conservando sus IDs; los locks retrasaron el traspaso. Son observaciones del diagnóstico, no ensayos repetidos para este informe. La implementación local se puede contrastar en [los hooks](../crates/tmux-aside-agents/src/main.rs) y [los argumentos de lanzamiento](../crates/tmux-aside/src/setup.rs).

El issue [#48500](https://github.com/openai/codex/issues/48500) seguía abierto. La reproducción inicial y los reportes de otros multiplexores pertenecen a usuarios. La respuesta de `etraut-openai`, marcada CONTRIBUTOR, confirma el modelo relevante: los hooks corren en el app-server; una conversación puede tener varios clientes o ninguno. No promete propagar identidad del terminal. [Respuesta del mantenedor](https://github.com/openai/codex/issues/48500#issuecomment-5878557125).

[#44902](https://github.com/openai/codex/issues/44902) propone contexto de cliente para hooks y seguía abierto. [#48880](https://github.com/openai/codex/issues/48880) solicita la conversación mostrada por cliente; está cerrado, pero sus comentarios y eventos consultados no muestran una implementación asociada: solo un comentario automático de posibles duplicados y un cierre sin commit. **Cerrado no equivale a funcionalidad disponible.**

## Codex: estado por panel sin abandonar el daemon

La TUI 0.169.0 escribe `OSC 7501;state=…:app=codex`, con estados `idle`, `working`, `blocked`, `done`, `error`; un bloqueo agrega `kind=permission`, `question` o `auth`. Al limpiar emite `state=clear`. No incluye ID de conversación, cliente o panel. El origen del flujo de bytes, si lo capturamos por panel, aporta la atribución visual que el entorno del daemon perdió. Esta última integración es una propuesta, no una función ya incorporada a tmux-aside. [Emisor versionado](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/terminal_program_status.rs).

Condiciones verificadas en código: stdout debe ser terminal; VTE anterior a 0.54 queda excluido. No hay una lista de `TERM_PROGRAM` permitidos para OSC 7501. Estar en tmux no lo desactiva. OSC 21337, distinto, está limitado a iTerm2 directo sin multiplexer ni `STY`. La caché evita repetir estados iguales y se invalida tras ceder el terminal: incorporar un observador tarde puede dejarlo sin valor inicial hasta la siguiente emisión. [Mismo emisor](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/terminal_program_status.rs).

Los tests upstream verifican inicio → trabajando → aprobación pendiente → trabajando → terminado → idle al reconocer el resultado; interrupción → idle y error → error. También distinguen resultados reproducidos del historial y trabajo posterior pendiente. No se ejecutaron esos tests upstream aquí. [Pruebas de estados](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/chatwidget/tests/program_status.rs), [prueba de aprobación y terminal en background](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/chatwidget/tests/terminal_program_status.rs).

**Prototipo propuesto:** consumir bytes originales con un observador por panel, conservar `unknown` hasta tener evidencia, traducir `blocked` a atención y limpiar al salir el proceso. No interpretar `done` como Criteria satisfechos. Un lector OSC puede resolver el indicador con daemon activo, pero no permite restaurar una conversación, correlacionar hooks por `session_id` ni conocer trabajo headless. Revisar conflicto con otros consumidores de `pipe-pane`, límites de buffers y privacidad antes de elegir ese transporte.

## Codex: identidad de conversación y estado del runtime

El esquema generado con el binario instalado ofrece `ThreadStatus` y `thread/status/changed`: `active.activeFlags` distingue `waitingOnApproval` y `waitingOnUserInput`. `thread/read`, `thread/list` y `thread/loaded/list` sirven para consultar conversaciones; ninguna lista prueba cuál está en pantalla en una TUI concreta. `originator` describe la creación; `attachedToThreadId` relaciona conversaciones, no terminales. Estos campos no deben usarse como identificadores de panel. [Esquemas versionados](https://github.com/openai/codex/tree/rust-v0.169.0/codex-rs/app-server-protocol), [selección interna de la TUI](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/app/thread_routing.rs).

La TUI tiene `current_displayed_thread_id()`, pero una función interna no es una API externa. No se encontró en las fuentes examinadas un contrato público completo `TUI PID → thread visible`, con cambios `/new`, `/resume`, fork y selección entre conversaciones cargadas. Tampoco OSC 7501 lo aporta. Es una conclusión acotada a las superficies revisadas, no una prueba de inexistencia de cualquier mecanismo interno. [Selección interna](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/app/thread_routing.rs), [pedido de interfaz](https://github.com/openai/codex/issues/48880).

**Lanzamiento controlado, viable como experimento:** tmux-aside crea una conversación con `thread/start`, guarda el ID devuelto y abre `codex resume <ID>` en un panel conocido; consulta estado por ese ID. Los métodos existen, pero falta verificar en la versión instalada la conexión al daemon correcto, la persistencia de una conversación sin turno y la continuidad entre ambos clientes. Un app-server separado no es automáticamente el runtime de la TUI. [Protocolo](https://github.com/openai/codex/tree/rust-v0.169.0/codex-rs/app-server-protocol), [CLI resume y no-daemon](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/cli.rs).

Ese registro inicial queda obsoleto si el usuario cambia de conversación dentro del mismo PID. Reanudar dos TUIs en el mismo ID crea una relación de varios paneles, no un dueño exclusivo. Los subagentes deben relacionarse con el Launch sin usurpar su selección visible. Para sesiones manuales o cambios internos hace falta un evento del cliente con ID completo, generación y orden, o una confirmación explícita de identidad. Es la garantía que solicita #48880; no basta inferirla de cwd, timestamps, último turno, historial o PID del daemon. [Requisitos del issue](https://github.com/openai/codex/issues/48880), [modelo multi-cliente explicado upstream](https://github.com/openai/codex/issues/48500#issuecomment-5878557125).

## Claude: aprovechar su superficie soportada

`claude agents --json --all` documenta `id` de job, `sessionId` completo, `kind`, y, si el proceso vive, `pid`, `status` y `waitingFor`; los jobs también tienen `state`. La documentación lo recomienda para observadores externos y desaconseja leer `state.json`. Para foreground, proponemos unir PID con la ascendencia del panel. Para background, el PID pertenece al worker del supervisor y no demuestra qué terminal está adjunta. [Referencia](https://code.claude.com/docs/en/agent-view#list-sessions-as-json).

`claude --bg` devuelve un ID administrable; `claude attach <id>` permite que un launcher conozca qué job abrió en cada panel. Los hooks aportan `session_id`; `SessionStart` cubre inicio/reanudación y los subagentes tienen `agent_id`. Mantener separados job, conversación y panel. [CLI](https://code.claude.com/docs/en/cli-reference), [hooks](https://code.claude.com/docs/en/hooks).

El supervisor conserva trabajo sin terminal y cada sesión corre en su propio proceso. `CLAUDE_CONFIG_DIR` separa instancias; `CLAUDE_JOB_DIR` identifica el directorio del job. Desactivar agent view elimina esa función; no representa una modalidad equivalente a `codex --no-daemon`. [Supervisor y configuración](https://code.claude.com/docs/en/agent-view#the-supervisor-process).

**Verificación local:** Claude 2.1.325 rechazó `--no-daemon`. `claude agents --help` anunció JSON para foreground y background; la ejecución real devolvió siete registros background bloqueados, con ID de job y sesión pero sin PID, tty ni panel. Un foreground observado no apareció. Por tanto, el contrato documentado permite una implementación parcial, pero la cobertura de las sesiones locales debe comprobarse antes de depender de él. No se deduce que toda sesión viva tenga registro ni que el PID observado sea el cliente adjunto.

## Costos de las alternativas

| Alternativa | Qué conserva o resuelve | Qué falta o cuesta |
|---|---|---|
| Codex compartido + OSC | Indicador procedente del panel; mantiene arquitectura daemon. | Integrar captura y estado inicial desconocido; sin ID, historia ni estado headless. |
| Codex compartido + registro al lanzar + RPC | Identidad inicial y estado por conversación. | Invalidar o actualizar cambios de selección; validar conexión y bootstrap. |
| Cliente propio sobre app-server | Puede conocer exactamente sus selecciones y eventos. | Proyecto mayor: interfaz, aprobaciones, compatibilidad y reconexión propias. |
| Daemon dedicado por panel | Hipótesis que elimina mezcla entre lanzamientos si se controla todo el ciclo de vida. | Recursos extra, sockets, configuración y credenciales; sigue sin resolver múltiples clientes ni subagentes. No probado. |
| Codex `--no-daemon` | Flag verificado que evita el servidor compartido incluso si ya corre. | Se renuncia al runtime compartido y a su independencia del cliente; no demuestra pérdida de todas las herramientas, MCP o subagentes. |
| Claude supervisor + IDs de job/sesión | Identidad administrable y estado documentado. | La identidad del cliente adjunto requiere registro aparte. |

Las filas de propuestas son evaluaciones de ingeniería. El alcance literal de `--no-daemon` procede de [la CLI 0.169.0](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/cli.rs); el modelo compartido de [la respuesta upstream](https://github.com/openai/codex/issues/48500#issuecomment-5878557125). La alternativa Claude se apoya en [su CLI](https://code.claude.com/docs/en/cli-reference). No se midió consumo ni se comprobó paridad funcional entre modos.

## Protocolo local propuesto y seguridad

Propuesta: guardar bindings separados `(servidor tmux, panel, generación del proceso, proveedor, conversación/job, fuente, última validación)`. Un evento de estado actualiza su conversación; solo actualiza el panel si existe un binding vigente. Una conversación puede tener cero, uno o varios paneles. La ausencia de identidad es `unknown`, nunca el panel heredado del daemon.

Para mostrar estado basta una fuente visual atribuida al flujo del panel. Para enviar texto o restaurar una sesión exigir evidencia vigente y destino inequívoco; un OSC es falsificable por cualquier proceso que escriba en ese PTY. Validar dueño y permisos del registro/socket, limitar tamaño de payload, no interpolar IDs en comandos de shell y no registrar prompts ni entornos completos. Invalidar al reiniciar o reutilizar un panel. Son medidas propuestas para esta integración, no garantías de los protocolos de proveedores.

La integración necesita una protección explícita en el Watcher: actualmente `is_idle` acepta un estado que no sea `working` ni `waiting`, incluido `None`, si coinciden PID y comando y nadie mira el panel. No verifica `inferred`. Por eso, representar `unknown` como el `None` actual podría habilitar un Nudge sin evidencia de disponibilidad. Debe conservarse la distinción entre desconocido e inactivo antes de alimentar automatizaciones con la nueva fuente. Además, `live_agent` del Watcher consulta el comando crudo de tmux; todavía no comparte la resolución del proceso hijo añadida al plugin de agentes. Son puntos de integración detectados, no corregidos por esta investigación. [Guarda y actividad](../crates/tmux-aside-watcher/src/lib.rs), [consulta del panel](../crates/tmux-aside-watcher/src/main.rs).

También habrá que definir precedencia entre fuentes: un hook antiguo de Codex que siga confiando en el `TMUX_PANE` del daemon no debe sobrescribir el estado atribuido por OSC a otro panel. Cambiar el lector visual sin resolver esa escritura deja abierta la causa original. [Escritura actual de hooks y reconciliación](../crates/tmux-aside-agents/src/main.rs).

## Plan de verificación antes de implementar

1. Usar tmux privado y cuentas/configuraciones temporales. Dos paneles, mismo cwd, daemon compartido; observar identidades sin cambiar sesiones de trabajo.
2. Codex: capturar OSC en inicio, turno, aprobación, `request_user_input`, autenticación, error, interrupción, reconocimiento y salida. Incorporar un observador después del inicio y verificar su estado desconocido inicial.
3. Probar `/new`, `/resume`, fork, X → Y → X sin prompts, dos clientes del mismo thread, subagentes y trabajo sin cliente. El indicador debe seguir su panel; la identidad no debe adivinarse.
4. RPC: consultar el runtime existente mediante transporte oficial; registrar versión de cliente y servidor, reconexión, snapshot y orden de notificaciones. Un fallo de transporte no prueba ausencia del método.
5. Claude: contrastar JSON con foreground recién iniciado y antiguo, job creado por `--bg`, attach/detach en otro panel, reinicio del worker y hooks con el mismo `session_id`.
6. Solo después comparar daemon y modo embebido: continuidad al cerrar TUI, aprobaciones, MCP, subagentes, latencia, memoria y recuperación. No migrar automáticamente sesiones existentes.

## Evidencia local adicional

Se ejecutó `/private/tmp/tmux-aside-research-osc.py` con un servidor tmux privado y dos paneles. Ambos emitieron señales sintéticas idénticas al formato de Codex 0.169.0, con `blocked:kind=permission` y `blocked:kind=question`, divididas entre varias escrituras. `pipe-pane -O` capturó cada flujo independientemente; `capture-pane` omitió OSC. Se limpió el servidor. **Prueba transporte y atribución por panel, no el recorrido completo desde una aprobación real de Codex con daemon.** Los artefactos temporales quedaron en `/var/folders/70/ywr6px350gz6pfrrknj9w_ch0000gn/T/tmux-aside-osc-research-zco6_rwc`; no son fixtures durables del repositorio.

`codex app-server daemon version` informó daemon activo 0.169.0 y socket de control `app-server-control.sock`. Un intento de `initialize` JSON por líneas directo al socket terminó en EOF; `codex app-server proxy` con el mismo formato no produjo respuesta en ocho segundos. El transporte/framing queda sin resolver: no demuestra que los métodos de estado sean inexistentes. Estas consultas y la prueba privada no modificaron sesiones ni configuraciones reales.

La selección del estado OSC da prioridad a un bloqueo del panel inferior; después a turno pendiente/en ejecución, tarea activa o goal todavía activo; finalmente al resultado anterior. Esto refuerza su utilidad para atención visible y también impide equiparar `done` con terminación de todo el trabajo coordinado. [Implementación versionada](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/chatwidget/status_surfaces.rs).

## Validación posterior: TUIs reales y servidor compartido

Se construyó [un prototipo descartable](../examples/prototypes/codex_osc.py), ejecutable con:

```sh
python3 examples/prototypes/codex_osc.py
```

Ejecuta dos TUIs del binario instalado de Codex 0.169.0 en tmux privado, con el mismo cwd y el mismo app-server. Ambas se conectan mediante `--remote unix://…`, sin `--no-daemon`; el app-server hereda deliberadamente el identificador del primer panel. `HOME`, `CODEX_HOME`, configuración y estado son temporales. Un servidor HTTP local devuelve respuestas fijas del protocolo Responses; no se usan credenciales ni inferencia remota. El catálogo usa la definición local de GPT-5.5 para exponer herramientas convencionales: no se está midiendo ni ejecutando ese modelo. [Prototipo, configuración y respuestas simuladas](../examples/prototypes/codex_osc.py).

Resultados observados y comprobados por el prototipo:

| Escenario | Señal recibida en el panel correspondiente |
|---|---|
| Inicio de ambas TUIs | `state=idle:app=codex` |
| Primer panel: llamada de herramienta que requiere permiso | `working` → `blocked:kind=permission` |
| Segundo panel: respuesta completada mientras el primero espera | `working` → `done`, sin `blocked` ajeno |
| Interrumpir la aprobación del primer panel sin ejecutarla | `working` → `idle` |
| `/new`, modo Plan y `request_user_input` | `working` → `blocked:kind=question`, sin contaminar el otro panel |
| Conectar un nuevo lector cuando el segundo panel ya terminó | Ninguna repetición del estado previo: inicialmente desconocido |
| Nuevo turno después de conectar ese lector | Recibe la siguiente transición hasta `done` |

La fuente de esta tabla es la ejecución del [prototipo](../examples/prototypes/codex_osc.py). Conserva capturas crudas y peticiones sintéticas en el directorio temporal que imprime y detiene los procesos privados al terminar. Los permisos simulados se cancelan; el comando solicitado no se aprueba.

**Veredicto:** el indicador por panel puede seguir permisos y preguntas reales de la TUI conservando un backend compartido. Ya no depende solo de una prueba con bytes sintéticos. Esto valida el mecanismo que necesita el sidebar; no aporta identidad de conversación ni autoriza Nudges automáticos.

**Límites:** el proceso compartido se inicia explícitamente con `codex app-server --listen`, no mediante la instalación/actualización automática del daemon administrado. No se probaron autenticación bloqueada, fallos de red, `/resume` y forks, varios clientes de la misma conversación, coexistencia con otro consumidor de `pipe-pane`, reconexión ni rendimiento. Las sesiones reales continúan con el workaround instalado. El siguiente cambio de producto debe resolver esos casos pertinentes, impedir que hooks antiguos sobrescriban la observación por panel y mantener `unknown` separado de `idle`.


## Implementación y validación del 2026-11-16

Se implementó la captura OSC por panel en `tmux-aside-agents`, ligada al socket tmux y PID del cliente. Un observador nuevo comienza desconocido; no se reutiliza el estado de otro proceso. Se conserva un consumidor `pipe-pane` preexistente y se invalidan desconexiones. Los hooks Codex antiguos se ignoran y `setup codex` los elimina sin tocar hooks ajenos. Los lanzamientos y reanudaciones vuelven al runtime predeterminado, sin forzar `--no-daemon`.

Watcher exige finalización confiable por hook para enviar Nudges. Las señales OSC son informativas: no contienen identidad de conversación y nunca autorizan acciones automáticas. Claude Code y Antigravity conservan sus hooks; reconocer visualmente un turno terminado no pierde su marca de finalización.

`examples/prototypes/codex_osc_managed.py` reproduce con Codex 0.169.0 un daemon gestionado aislado, dos TUIs reales, un proveedor HTTP local simulado y el binario compilado del plugin. Verificó permisos, preguntas, interrupción, conversación nueva y captura tardía; finalmente el plugin mostró `waiting` y `done` independientes con dos PID distintos. No usa credenciales ni modifica sesiones habituales. Evidencia de ejecución: `/private/tmp/osc-managed-plugin-clean.log` (directorio de artefactos indicado por el script).

Las pruebas de integración cubren aislamiento por panel, hooks antiguos, consumidor preexistente, sustitución del pipe, salida del plugin y reinicio en el mismo panel. Las pruebas puras cubren registros partidos/malformados/acotados, identidad del servidor/proceso y rechazo de Nudges para estados desconocidos/inferidos/OSC.


Instalado localmente: binarios `tmux-aside`, `tmux-aside-agents` y `tmux-aside-watcher`; hooks Codex obsoletos retirados y sidebar recargado sin reiniciar clientes. Respaldo de binarios: `/private/tmp/tmux-aside-pre-osc-9o2vf5bl`. Se verificaron cinco observadores de clientes reales y una transición a `waiting`; los clientes sin una nueva emisión permanecen desconocidos.

Validación final: build debug/release, formato y clippy pasan. La suite completa detectó dos timeouts conocidos de `shared_view` (también fallan individualmente y ya se habían reproducido en la base). La expectativa antigua de instalación/desinstalación de hooks Codex se actualizó; sus siete pruebas de desinstalación y doce pruebas de setup pasan tras corregir la creación innecesaria de `hooks.json` vacío. El resto de suites pasó.
