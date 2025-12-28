Interpolation Engine is a CLI tool to execute *programs* defined by JSON5 files.

**Why JSON5?** Valid *programs* are a subset of JSON5. JSON is unambiguous, easy to parse, fast to parse, and easy to write for experienced programmers. It is can express the nested structures that Interpolation Engine requires. I use JSON5 because I want comments, trailing commas, and optional quotes for keys.

## Command Reference

This is a list of all valid Interpolation Engine commands.

- `print`
  - Fields: `text`
  - Appends text to output and writes it to the screen.
  - Example:
    ```json5
    {cmd: "print", text: "Hello\n"}
    ```

- `clear`
  - Clears output and the screen.
  - Example:
    ```json5
    {cmd: "clear"}
    ```

- `sleep`
  - Fields: `seconds`
  - Sleeps for the given duration. Accepts numbers or a math expression string.
  - Example:
    ```json5
    {cmd: "sleep", seconds: 0.5}
    ```

- `set`
  - Fields: `item`, `output_name`
  - Stores `item` under `output_name` in `state.inserts`.
  - Example:
    ```json5
    {cmd: "set", item: "Tom", output_name: "name"}
    ```

- `unescape`
  - Fields: `item`, `output_name`
  - Like `set`, but unescapes `\{` and `\}` and re-interpolates.
  - Example:
    ```json5
    {cmd: "unescape", item: "Use \\{name\\}", output_name: "text"}
    ```

- `show_inserts`
  - Shows the current `state.inserts`.
  - Example:
    ```json5
    {cmd: "show_inserts"}
    ```

- `random_choice`
  - Fields: `list`, `output_name`
  - Picks a random element from `list`.
  - Example:
    ```json5
    {cmd: "random_choice", list: ["red", "green"], output_name: "color"}
    ```

- `join_list`
  - Fields: `list`, `before`, `between`, `after`, `output_name`
  - Joins list items into a string with prefix/suffix.
  - Example:
    ```json5
    {cmd: "join_list", list: [1, 2, 3], before: "[", between: ", ", after: "]", output_name: "nums"}
    ```

- `list_concat`
  - Fields: `lists`, `output_name`
  - Concatenates a list of lists.
  - Example:
    ```json5
    {cmd: "list_concat", lists: [[1], [2, 3]], output_name: "all"}
    ```

- `list_append`
  - Fields: `list`, `item`, `output_name`
  - Appends `item` to `list` and stores the result.
  - Example:
    ```json5
    {cmd: "list_append", list: [1, 2], item: 3, output_name: "all"}
    ```

- `list_remove`
  - Fields: `list`, `item`, `output_name`
  - Removes the first matching `item` from `list` if present.
  - Example:
    ```json5
    {cmd: "list_remove", list: [1, 2, 2], item: 2, output_name: "rest"}
    ```

- `list_index`
  - Fields: `list`, `index`, `output_name`
  - 1-based indexing; negative indices count from the end.
  - Example:
    ```json5
    {cmd: "list_index", list: ["a", "b", "c"], index: -1, output_name: "last"}
    ```

- `list_slice`
  - Fields: `list`, `from_index`, `to_index`, `output_name`
  - 1-based, right-inclusive slicing; indices may be math expressions.
  - Example:
    ```json5
    {cmd: "list_slice", list: [1, 2, 3, 4], from_index: 2, to_index: 3, output_name: "mid"}
    ```

- `user_input`
  - Fields: `prompt`, `output_name`
  - Prompts the user; input is escaped before storing.
  - Example:
    ```json5
    {cmd: "user_input", prompt: "Name? ", output_name: "name"}
    ```

- `user_choice`
  - Fields: `list`, `description`, `output_name`
  - Presents a list to the user and stores the chosen item.
  - Example:
    ```json5
    {cmd: "user_choice", list: ["small", "large"], description: "Size", output_name: "size"}
    ```

- `label`
  - Fields: `name`
  - Defines a label for `goto` and `goto_map`.
  - Example:
    ```json5
    {cmd: "label", name: "@start"}
    ```

- `goto`
  - Fields: `name`
  - Jumps to a label. Not supported inside `parallel_*` tasks.
  - Example:
    ```json5
    {cmd: "goto", name: "@start"}
    ```

- `goto_map`
  - Fields: `text`, `target_maps`
  - Conditional goto. `target_maps` is a list of single-entry dicts mapping patterns (with `*` wildcards) to label names. Supports `NULL` key when interpolation fails. Not supported inside `parallel_*` tasks.
  - Example:
    ```json5
    {cmd: "goto_map", text: "{user_input}", target_maps: [{"yes": "@ok"}, {"*": "@fallback"}]}
    ```

- `replace_map`
  - Fields: `item`, `output_name`, `wildcard_maps`
  - Optional: `repeat_until_done` (bool)
  - Applies wildcard pattern replacements; supports `NULL` key for interpolation errors.
  - Example:
    ```json5
    {cmd: "replace_map", item: "Age 41", output_name: "age", wildcard_maps: [{"Age *": "{1}"}]}
    ```

- `for`
  - Fields: `name_list_map`, `tasks`
  - Iterates lists in lockstep and runs `tasks` for each iteration. Lists must be the same length.
  - Example:
    ```json5
    {cmd: "for", name_list_map: {"name": ["A", "B"]}, tasks: [{cmd: "print", text: "{name}\n"}]}
    ```

- `serial`
  - Fields: `tasks`
  - Runs nested tasks sequentially.
  - Example:
    ```json5
    {cmd: "serial", tasks: [{cmd: "print", text: "A"}, {cmd: "print", text: "B"}]}
    ```

- `parallel_wait`
  - Fields: `tasks`
  - Runs tasks concurrently and waits for all to finish.
  - Example:
    ```json5
    {cmd: "parallel_wait", tasks: [{cmd: "sleep", seconds: 1}, {cmd: "sleep", seconds: 1}]}
    ```

- `parallel_race`
  - Fields: `tasks`
  - Runs tasks concurrently and cancels the others once one finishes.
  - Example:
    ```json5
    {cmd: "parallel_race", tasks: [{cmd: "sleep", seconds: 1}, {cmd: "sleep", seconds: 2}]}
    ```

- `run_task`
  - Fields: `task_name`
  - Runs a task from `program.tasks` by name. Extra fields are passed through.
  - Example:
    ```json5
    {cmd: "run_task", task_name: "my_task"}
    ```

- `delete`
  - Fields: `wildcards`
  - Deletes inserts matching wildcard patterns.
  - Example:
    ```json5
    {cmd: "delete", wildcards: ["tmp/*"]}
    ```

- `delete_except`
  - Fields: `wildcards`
  - Deletes all inserts except those matching wildcard patterns.
  - Example:
    ```json5
    {cmd: "delete_except", wildcards: ["user/*"]}
    ```

- `math`
  - Fields: `input`, `output_name`
  - Evaluates a math expression and stores the result.
  - Example:
    ```json5
    {cmd: "math", input: "3 + 4 * 2", output_name: "result"}
    ```

- `chat`
  - Fields: `messages`, `output_name`
  - Optional: `model` (required unless program-level `completion_args` supplies it)
  - Other optional args: `n_outputs`, `start_str`, `stop_str`, `hide_start_str`, `hide_stop_str`, `shown`, `choices_list_name`, `choices_list`, `extra_body`, `max_completion_tokens`, `temperature`, `seed`, `stop`
  - Example:
    ```json5
    {cmd: "chat", messages: [{role: "user", content: "Hi"}], output_name: "reply", model: "gpt-4o-mini"}
    ```

- `generate`
  - Fields: `prompt`, `output_name`
  - Optional: `model` (required unless program-level `completion_args` supplies it)
  - Other optional args: `n_outputs`, `start_str`, `stop_str`, `hide_start_str`, `hide_stop_str`, `shown`, `num_ctx`, `repeat_last_n`, `repeat_penalty`, `temperature`, `seed`, `stop`, `num_predict`, `top_k`, `top_p`, `min_p`
  - Example:
    ```json5
    {cmd: "generate", prompt: "Say hello", output_name: "reply", model: "llama3"}
    ```
