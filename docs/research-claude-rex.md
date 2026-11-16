# Dirección de tmux-aside ante Claude Code y Rex

Análisis del 14 de noviembre de 2026. Propuesta para discutir; no modifica las decisiones vigentes ni implica una migración aprobada.

Recomiendo fortalecer la coordinación del Lifecycle, conservar tmux y separar progresivamente la ejecución de agentes de la interfaz. Una futura integración con Rex debe probarse con un recorrido pequeño antes de generalizar la arquitectura.

## Alcance de las fuentes

Para [Rethinking Claude Code Best Practices, Lydia Hallie](https://www.youtube.com/watch?v=vlfCjWuv4es), se consultó la [transcripción del organizador](https://ai.engineer/code/2026/talks/lydia-hallie). Es automática y puede contener errores de reconocimiento.

El segundo enlace corresponde a [Rex is Probably The Best Terminal Emulator I’ve Ever Used, DevOps Toolbox](https://www.youtube.com/watch?v=DA1uyLPwuVQ). Se verificaron título, autor y descripción en la página de YouTube, pero no se obtuvo su transcripción: el recurso de subtítulos respondió vacío y el endpoint de transcripción rechazó la consulta. Por tanto, las conclusiones sobre Rex se apoyan en documentación primaria del producto, sin atribuir al video demostraciones o afirmaciones no verificadas. No se probó Rex instalado.

## Lo que aporta cada referencia

Lydia describe una sesión principal que divide trabajo, inicia otras sesiones, evalúa resultados y devuelve lo incompleto (15:10–16:28). También destaca verificaciones intermedias, CI y revisión con contexto nuevo. Para tmux-aside, la consecuencia propuesta es ofrecer operaciones fiables a ese coordinador y evidencia verificable de avance. La charla no demuestra que debamos reemplazar nuestro Watcher por un modelo. [Transcripción](https://ai.engineer/code/2026/talks/lydia-hallie).

Rex proporciona terminales persistentes, control programático y acceso remoto. Su CLI permite listar estructuras en JSON, enviar texto y capturar pantallas; la API del servidor permite descubrir métodos y esquemas. Un adaptador resulta plausible, pero depende de las capacidades reales de la versión instalada. [CLI](https://www.superlogical.com/rex/docs/automate/cli), [API](https://www.superlogical.com/rex/docs/automate/server-api).

El solapamiento con nuestra interfaz es real: Rex documenta Activity y agrupación por prioridad, además de indicadores y notificaciones. Su skill permite que agentes operen pestañas y paneles. Las fuentes consultadas no establecen un Lifecycle propio de PRs equivalente al nuestro; esas primitivas sí permiten construir coordinación encima. [Acciones](https://www.superlogical.com/rex/docs/reference/mac-actions), [Configuración](https://www.superlogical.com/rex/docs/reference/mac-settings), [Agentes](https://www.superlogical.com/rex/docs/automate/agent-skill).

OSC 7501 comunica estados de programas y motivos de bloqueo. Es una posible vía de interoperabilidad, pero no acredita que una tarea cumpla sus criterios ni permite consultar registros a través del propio protocolo. El transporte a través de tmux y la atribución a cada panel interior requieren pruebas. [Especificación](https://www.superlogical.com/rex/docs/build/program-status).

## Lo que ya tenemos

El [modelo de dominio](../CONTEXT.md) ya define coordinación: Launch, Group, Lifecycle, Criterion, Watcher y Nudge. El [README](../README.md) documenta worktrees, varios agentes, QA por Group, Packs, recuperación y limpieza. Presentarnos únicamente como sidebar deja fuera gran parte del producto actual.

La [decisión arquitectónica vigente](adr/0001-fixed-lifecycle-with-pack-criteria.md) eligió un Lifecycle fijo y rechazó convertirse en un motor genérico de etapas. Esta propuesta conserva esa restricción. Un coordinador puede decidir qué trabajo iniciar y cómo dividirlo sin redefinir las etapas por las que pasan sus PRs.

El valor a validar es: reducir las intervenciones necesarias para llevar cambios de varios repositorios a producción bajo las reglas del equipo. El soporte de varios proveedores ayuda, pero es insuficiente como diferenciador por sí solo.

## Hallazgos concretos y mejoras propuestas

| Hallazgo actual | Cambio propuesto | Verificación útil |
|---|---|---|
| El daemon inicia los plugins y los detiene al terminar el período sin clientes. | Dar al seguimiento de Launches una vida independiente del sidebar, con inicio y detención explícitos. | Ocultar todos los sidebars y comprobar que el Watcher continúa evaluando y actuando. |
| El Criterion predeterminado muestra CI fallido, pero solo propone Nudges por conflictos y comentarios pendientes. | Añadir reparación de CI con evidencia del fallo y límites de intentos. | Un fallo reparable produce una corrección; un fallo repetido se escala sin bucle ilimitado. |
| La clave de deduplicación es `stage@head`; se registra antes de enviar el Nudge. | Conservar la protección contra duplicados, añadiendo causa, intento y resultado de entrega. | Un fallo de envío queda visible y recuperable; no se repite ciegamente una acción potencialmente entregada. |
| Launch persiste `session`, `window` y `pane`; lanzamiento, recuperación, limpieza y envío usan tmux directamente. | Introducir una frontera pequeña para abrir, localizar, enviar y cerrar agentes cuando se pruebe un segundo backend. | El mismo recorrido funciona con tmux y el prototipo de Rex sin condiciones de backend en Criteria. |
| El template de QA termina usando `tmux kill-pane`. | Encapsular también las operaciones de cierre que reciben los agentes en sus instrucciones. | QA no necesita conocer el multiplexer usado. |

Fuentes locales: [daemon](../crates/tmux-aside/src/daemon.rs), [Criterion y deduplicación](../crates/tmux-aside-watcher/src/lib.rs), [envío de Nudges](../crates/tmux-aside-watcher/src/main.rs), [Launch](../crates/tmux-aside/src/launch.rs), [template QA](../crates/tmux-aside/templates/qa.md).

La actividad del agente y el estado del trabajo deben seguir siendo conceptos distintos. Terminar un turno habilita una evaluación; no demuestra que el cambio esté verificado. Añadiría referencias a resultados, commit evaluado y antigüedad de la evidencia, manteniendo opcionales los nuevos campos del protocolo.

La vista de atención debería explicar qué decisión se necesita y por qué el sistema no puede avanzar: merge pendiente, credencial, fallo repetido, dependencia o QA fallido. Reutilizaría la prioridad y navegación existentes. Mediría las interrupciones útiles y falsas, no la cantidad de indicadores.

## Decisión sobre orquestación y tmux

Hay dos responsabilidades distintas:

- El coordinador con IA interpreta objetivos, decide una división del trabajo y juzga resultados ambiguos. Puede ser una sesión de Claude o Codex que utiliza nuestra CLI y skills.
- tmux-aside conserva estado durable, aplica Criteria, coordina Groups y entrega trabajo a agentes. Sus decisiones reproducibles siguen en código.

Construir un planificador cognitivo propio ahora añade complejidad y competencia directa con los proveedores sin evidencia de necesidad. Primero conviene identificar qué operaciones faltan para que un agente existente pueda coordinar el Lifecycle con fiabilidad.

Mantendría tmux como integración principal y la experiencia actual. Una integración Rex sería adicional. No implementaría un emulador de terminal, un servicio de acceso remoto ni una aplicación móvil para competir con Rex. La independencia del núcleo tiene utilidad incluso si nunca incorporamos otro frontend.

El Watcher independiente tampoco garantiza ejecución durante la suspensión o apagado del equipo. Eso requiere un host disponible y una estrategia de recuperación. Rex distingue igualmente persistencia al desconectar el cliente de reinicios del servidor que pueden terminar programas. [Actualizaciones de servidores](https://www.superlogical.com/rex/docs/linux/upgrading).

## Experimentos antes de un pivot

1. Registrar una línea base de 10–20 cambios reales: intervenciones manuales, causa del bloqueo, tiempo detenido y retrabajo. Es una muestra exploratoria propuesta, no un benchmark estadístico.
2. Separar seguimiento y sidebar; añadir evidencia y una reparación acotada de CI. Comparar con trabajos similares, incluyendo costo y fallos, no solo cantidad de PRs.
3. Usar una sesión coordinadora existente sobre nuestra CLI para un Group que cruza dos repositorios. Comprobar que puede iniciar, observar, pedir correcciones y reconocer cuándo requiere decisión humana.
4. Prototipar en Rex únicamente lanzar un agente, observar estado, entregar un Nudge y volver a su terminal. Descubrir las APIs en el servidor real; verificar identidad, desconexiones y recuperación. Mantener los hooks silenciosos y stdout de plugins como JSON.

Adoptaría Rex si elimina fricción cotidiana y preserva nuestras garantías. Ampliaría la orquestación si reduce intervenciones sin aumentar retrabajo ni costo desproporcionadamente. Consideraría abandonar tmux únicamente con evidencia de que dejó de servir a los usuarios y que sostenerlo bloquea el producto. Los videos, por sí solos, no justifican esa decisión.
