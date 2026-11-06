# Flag handshakes and `--help` (recorded)

CLI `2026.09.15-d2fe57e`, native Windows, launched as `node.exe index.js <args>` with `NO_OPEN_BROWSER=1`, one `initialize` line on stdin, then stdin closed. Raw exchanges: `flag-*.jsonl`.

| Arguments | Result |
| --- | --- |
| `acp` | `initialize` answered, exit 0 (`initialize.jsonl`) |
| `--model composer-2.5 acp` | `initialize` answered, exit 0 |
| `--mode=plan acp` | `initialize` answered, exit 0 |
| `--plan acp` | `initialize` answered, exit 0 |
| `--mode=bogus acp` | no frame; stderr `error: option '--mode <mode>' argument 'bogus' is invalid. Allowed choices are plan, ask.`; exit 1 |

A handshake shows only that the flag parses before `acp`. On the existing chat, `plan-load.jsonl` loads with `--mode=plan` and the session result stays `currentModeId: "agent"`; `session/set_mode` to `plan` is what switches it (NOTES.md).

## `agent --help` findings

- `--mode <mode>`: choices `plan`, `ask` only (there is no `--mode=agent`; Agent is the default).
- `--plan`: shorthand for `--mode=plan`.
- `--model <model>`: “Parameterized models accept quoted bracket overrides, e.g. 'claude-opus-4-8[context=1m,effort=high,fast=false]'”.
- `--list-models`, `models`, `login` (“Set NO_OPEN_BROWSER to disable browser opening.”), `logout`, `status|whoami`, `about`, `update`, `create-chat`, `ls`, `resume`, `--resume [chatId]`, `--continue`, `-f/--force`, `--yolo`, `--auto-review`, `--sandbox`, `--approve-mcps`, `--trust`, `--workspace`, `-p/--print`, `worker`, `persist`.
- `acp` is not listed under Commands, but `agent acp` runs the ACP server.
- No command deletes a chat (`ls` and `resume` only list and reopen).
