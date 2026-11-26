# Agentes y ejecutores

`src/agents.rs::PROVIDERS` es un slice: agregar un CLI requiere un bloque `Provider`.
`binary` sirve para detectarlo y `command` abre su sesión desde la paleta.
`install` contiene una URL oficial de instalación; Orion no instala agentes automáticamente.
`executor` solo se define si existe una forma verificada de enviar el prompt inicial y
mantener una sesión interactiva. `use_for` describe su utilidad al planificador.

El catálogo incluye los providers con plantilla y filtra los binarios disponibles antes
de planificar, con la misma detección de instalaciones rotas que la paleta. La presencia
del binario no comprueba credenciales, modelos ni la configuración propia del CLI.
Claude Code sigue siendo necesario para planificar.

## Referencias de los comandos

Revisadas el 26 de noviembre de 2026. Son comprobaciones documentales: los tests de Orion
no ejecutan consultas reales a estos servicios.

| CLI | Plantilla del ejecutor | Documentación oficial |
| --- | --- | --- |
| Claude Code | `claude {prompt}` | [Referencia CLI](https://code.claude.com/docs/en/cli-reference) |
| Gemini CLI | `gemini --prompt-interactive {prompt}` | [Configuración](https://geminicli.com/docs/reference/configuration/) |
| Codex | `codex {prompt}` | [Referencia CLI](https://developers.openai.com/codex/cli/reference) |
| OpenCode | `opencode --prompt {prompt}` | [Referencia CLI V1, TUI](https://opencode.ai/v1/docs/cli/) |
| Qwen Code | `qwen --prompt-interactive {prompt}` | [Argumentos CLI](https://github.com/QwenLM/qwen-code/blob/main/docs/users/configuration/settings.md) |
| Copilot | `copilot -i {prompt}` | [Referencia CLI](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference) |
| Cursor Agent | `cursor-agent {prompt}` | [Sesión interactiva con prompt inicial](https://cursor.com/docs/cli/overview), [instalación](https://cursor.com/docs/cli/installation) |
| Goose | `goose run --interactive --text {prompt}` | [Tareas y continuación interactiva](https://github.com/aaif-goose/goose/blob/main/documentation/docs/guides/running-tasks.md) |
| Aider | Sin ejecutor: `--message` termina después de responder | [Opciones](https://aider.chat/docs/config/options.html) |
| Crush | Sin ejecutor: no se verificó una entrada inicial interactiva; `run` es no interactivo | [Comando raíz](https://github.com/charmbracelet/crush/blob/main/internal/cmd/root.go) |
| Amp | Sin ejecutor: la entrada inicial documentada usa stdin; no se verificó un argumento equivalente | [CLI](https://ampcode.com/docs/cli) |
| Kimi CLI | Sin ejecutor: `--prompt` termina después de responder | [Referencia CLI](https://moonshotai.github.io/kimi-cli/en/reference/kimi-command.html) |
| Ollama | Sin ejecutor: `ollama launch` abre el selector de integraciones | [Referencia CLI](https://docs.ollama.com/cli) |

Ollama no fija un modelo para el usuario. DeepSeek puede usarse mediante la configuración
de OpenCode, Aider u otros clientes; no se presupone un binario `deepseek`.

## Configuración y precedencia

`auto_executors = true` es el valor predeterminado, también al leer un config anterior.
Con `false` se omiten los providers automáticos; siguen los tres modelos de Claude,
los perfiles y los bloques `[[executor]]` explícitos.

Un solo perfil puede servir para abrir una pestaña y ejecutar subtareas:

```toml
[[agent]]
name = "OpenCode DeepSeek"
command = "opencode -m deepseek/deepseek-v4-pro"
worktree = true
use_for = "programar con el modelo DeepSeek configurado en OpenCode"
executor = "opencode -m deepseek/deepseek-v4-pro --prompt {prompt}"
# id = "deepseek" # opcional; por defecto: opencode-deepseek
```

El modelo del ejemplo debe existir en la configuración del cliente. `command` se usa
en la paleta; `executor` recibe el prompt de la subtarea. El orquestador crea siempre
un worktree por subtarea, independientemente de `worktree` del perfil.
Para entrar al catálogo, un perfil necesita tanto `use_for` como `executor`.

El id implícito es `agents::slug(name)`: minúsculas, letras y números Unicode,
separadores convertidos a un guion, sin guiones iniciales o finales. Si no quedan
letras ni números, usa `agent`. Un `id` explícito se conserva tal cual.

Para un mismo id gana, en este orden: **Claude < providers < perfiles < executors**.
Dentro de cada capa gana la última definición. Los reemplazos mantienen su posición
en el catálogo y nunca producen ids duplicados. `[[executor]]` conserva sus campos
`id`, `use_for` y `command` y su comportamiento anterior.

`orchestrator::catalog_with_sources(&settings)` devuelve cada ejecutor junto con
`ExecutorSource::{Claude, Detected, Profile, Executor}`. La procedencia refleja la
definición ganadora; `Detected` es el origen del provider, y no garantiza que esté
instalado. `catalog` devuelve las mismas entradas sin esa metadata.
