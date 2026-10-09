# Telamon Gates: roadmap

Gates is for story writing, everyday chat, coding and running a fleet of
subagents. It aims to be simple and still powerful: each feature below earns
its place, and the ones left out are listed with the reason.

## Kept, in build order

1. **SystemOne** (on by default, can be turned off in Settings). This is a
   small decision model (System 1) that answers typed questions about each
   message, and answers them fast. Gates acts on the answers before the chat
   model (System 2) writes anything. It runs on its own llama-server,
   through llama.cpp's `/v1/systemone` (TypeSafe-compatible, in
   telamon-llama 0.6.0).
   - **No Fleet:** Laya-421M (`ggml-org/Laya-GGUF`), on the processor, so it
     takes no video memory. Its context is 512 tokens per question.
   - **Fleet:** Kev-4B (`ggml-org/Kev-4B-GGUF`, Q4_K_M, 3 GB) on the graphics
     card. Its context is 8,192 tokens.
   - **First uses:**
     - Pick the mode for a message (Chat, Story, Code), so the right preset
       answers.
     - Decide whether a message needs a tool, and which one.
     - In Fleet, route a task to a subagent and judge an agent's state
       (working, stuck, done, needs you).
   - **Caveat (from Laya's own card):** Laya is a base to fine-tune, and it is
     near chance zero-shot. Kev is built for typed decisions over documents.
     Each question gets a test set before it acts on anything. A low-confidence
     answer falls back to the default (Chat, no tool), and the user can
     always override.
   - **Built:** picking the mode, with its test set. Laya is 89% right as
     used; Kev is 94% right in 47 ms on the RX 7900 (`docs/BACKEND.md`).
2. **Modes.** Chat, Story and Code ship by default. Each is a system prompt
   plus sampling settings: temperature, top-p and max tokens. Story runs warmer
   and longer; Code runs cooler. Modes can be edited and you can add your own;
   that is the prompt library. SystemOne picks one per message, or you pin a
   mode for a conversation.
   - **Built:** the three modes, with their temperature and top-p, and Auto
     or a pinned mode per conversation.
   - **Not yet:** editing modes and adding your own.
3. **Edit and Branch.** Edit a sent message, or "Branch From Here" on any
   message, to get a new conversation up to that point. This lets you try a
   story another way without losing the first. Export a conversation as
   Markdown or JSON.
   - **Built.**
4. **Model facts.** Each model's context length, whether it reads images (a
   matching `mmproj`) and whether its chat template takes tools. These come
   from the GGUF header and show on the Models page and in the picker.
5. **Tools, our own.** A Rust registry in gates-core: no MCP, no Python, and
   nothing to spawn per call. Tools go through llama-server's OpenAI
   `tools`/`tool_calls` with `--jinja`.
   - **Read-only tools run at once:** read a file in a folder you allowed,
     list a folder, search the conversations, the date and time, and
     arithmetic.
   - **Tools that change things ask first.** Model output is untrusted, so
     the app asks you each time.
   - **Built**, as Agent mode:
     - **Reading:** list, read, search and find files in the conversation's
       folder.
     - **Changing:** write and edit files, and run commands, each after
       you allow it.
     - **Tested:** with Qwen3-4B it did a small coding task in 6 steps and
       5.5 s (`docs/BACKEND.md`).
     - **Not yet:** the date, arithmetic, and searching conversations.
6. **Fleet.** A dashboard page in the Telamon style showing:
   - each subagent as a card: its model, its task, a live status from
     SystemOne, tokens/s and context used;
   - a timeline of what each agent did, with Stop, Pause and Talk To on each.
   It runs on the same llama-server with `--parallel N` slots.
   - **Built**, simply:
     - **The plan:** a goal and a folder. The chat model, asked once for a
       JSON plan, splits the goal into 1 to 6 tasks, and each runs as an
       agent in that folder, one after another on the one server slot.
     - **The page:** a card for each agent with its status (idle, working,
       waiting for you, done, failed, stopped), last line, steps and tokens
       per second; Stop for one agent or all; every change an agent asks for
       waits on the page for your answer.
     - **SystemOne:** when an agent ends, a decision model is asked "Did
       this agent complete its task?" and the card shows how sure it is.
       With no decision model, nothing is asked.
     - **Tested:** with Qwen3-4B and Laya, a two-task goal ran in 6 to 7 s
       and SystemOne said 85% and 75% sure (`docs/BACKEND.md`).
     - **Not yet:** `--parallel N` (agents at the same time), Pause, Talk To,
       a timeline, each agent's model and context used, saved runs, and
       routing a task to an agent by SystemOne.
7. **Attachments.** Text and code files go into the message. Retrieval with
   embeddings over many documents comes later, if attachments prove too
   small.
8. **Images.** Attach an image for models with a projector (`--mmproj`).

## Left out

- **MLX and ONNX.** MLX is Apple-only; ONNX doesn't run in llama.cpp. GGUF
  covers it.
- **Encrypted sync and multi-user.** Gates is a desktop app; sync belongs
  to Telamon OS.
- **Tags.** Search and branches cover it with less to manage.
- **Stop sequences in the UI.** The chat templates already end replies.
- **LaTeX, voice input, web search.** Maybe later, as tools: each needs
  another engine or the network.
