# Published Cursor ACP extension schemas

Source: https://cursor.com/docs/cli/acp (fetched 2026-11-02). These are the published request shapes. They are not recordings.

`cursor/create_plan` and `cursor/update_todos` were raised on the existing chat (`plan-load.jsonl`). Their field names in that fixture match the published names below. `cursor/ask_question` was not raised; the prompt and the turn result are in `NOTES.md`. `cursor/task` and `cursor/generate_image` did not appear in the follow-up prompts. The plan freezes those three unrecorded methods from this note.

## `cursor/ask_question` (blocking)

```ts
interface CursorAskQuestionRequest {
  toolCallId: string;
  title?: string;
  questions: Array<{
    id: string;
    prompt: string;
    options: Array<{ id: string; label: string }>;
    allowMultiple?: boolean;
  }>;
}
```

Response `outcome`: `{ outcome: "answered", answers: [{ questionId, selectedOptionIds }] }`, `{ outcome: "skipped", reason? }`, or `{ outcome: "cancelled" }`.

## `cursor/create_plan` (blocking)

```ts
interface CursorCreatePlanRequest {
  toolCallId: string;
  name?: string;
  overview?: string;
  plan: string;
  todos: Array<{ id: string; content: string; status: "pending" | "in_progress" | "completed" | "cancelled" }>;
  isProject?: boolean;
  phases?: Array<{ name: string; todos: Array<{ id: string; content: string; status: "pending" | "in_progress" | "completed" | "cancelled" }> }>;
}
```

The recorded requests (`plan-load.jsonl`) carried `toolCallId`, `name`, `overview`, `plan`, `todos` (`id`, `content`, `status`), `isProject`, and `phases`.

The recorded accept (`plan-load.jsonl`) is `{"outcome":{"outcome":"accepted"}}`. The recorded deny (`plan-deny.jsonl`) is `{"outcome":{"outcome":"rejected"}}`, with no `reason` field. The CLI completed the tool call and ended the turn `end_turn`. No other deny body was sent.

## `cursor/update_todos` (published as a notification)

```ts
interface CursorUpdateTodosRequest {
  toolCallId: string;
  todos: Array<{ id: string; content: string; status: "pending" | "in_progress" | "completed" | "cancelled" }>;
  merge: boolean;
}
```

The recorded frame was a JSON-RPC request with an id, `merge: false`, and one todo. The published page calls this a notification that needs no response. The plan answers when an id is present.

## `cursor/task` (published as a notification)

```ts
interface CursorTaskRequest {
  toolCallId: string;
  description: string;
  prompt: string;
  subagentType: "unspecified" | "computer_use" | "explore" | "video_review" | "browser_use" | "shell" | "vm_setup_helper" | { custom: string };
  model?: string;
  agentId?: string;
  durationMs?: number;
}
```

## `cursor/generate_image` (published as a notification)

```ts
interface CursorGenerateImageRequest {
  toolCallId: string;
  description: string;
  filePath?: string;
  referenceImagePaths?: string[];
}
```
