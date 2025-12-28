Interpolation Engine is a CLI tool to execute programs defined by JSON5 files.

**Why JSON5?** Valid programs are a subset of JSON5. JSON is unambiguous, easy to parse, fast to parse, and easy to write for experienced programmers. It is can express the nested structures that Interpolation Engine requires. I use JSON5 because I want comments, trailing commas, and optional quotes for keys.

## Command Reference

This is a list of all valid Interpolation Engine commands.

#### `print`
Fields: `text`<br>
Appends text to output and writes it to the screen.<br>
Example:<br>
```json5
{cmd: "print", text: "Hello\n"}
```

#### `clear`
Clears output and the screen.<br>
Example:<br>
```json5
{cmd: "clear"}
```

#### `sleep`
Fields: `seconds`<br>
Sleeps for the given duration. Accepts numbers or a math expression string.<br>
Example:<br>
```json5
{cmd: "sleep", seconds: 0.5}
```

#### `set`
Fields: `item`, `output_name`<br>
Stores `item` under `output_name` in `state.inserts`.<br>
Example:<br>
```json5
{cmd: "set", item: "Tom", output_name: "name"}
```

#### `unescape`
Fields: `item`, `output_name`<br>
Like `set`, but unescapes `\{` and `\}` and re-interpolates.<br>
Example:<br>
```json5
{cmd: "unescape", item: "Use \\{name\\}", output_name: "text"}
```

#### `show_inserts`
Shows the current `state.inserts`.<br>
Example:<br>
```json5
{cmd: "show_inserts"}
```

#### `random_choice`
Fields: `list`, `output_name`<br>
Picks a random element from `list`.<br>
Example:<br>
```json5
{cmd: "random_choice", list: ["red", "green"], output_name: "color"}
```

#### `join_list`
Fields: `list`, `before`, `between`, `after`, `output_name`<br>
Joins list items into a string with prefix/suffix.<br>
Example:<br>
```json5
{cmd: "join_list", list: [1, 2, 3], before: "[", between: ", ", after: "]", output_name: "nums"}
```

#### `list_concat`
Fields: `lists`, `output_name`<br>
Concatenates a list of lists.<br>
Example:<br>
```json5
{cmd: "list_concat", lists: [[1], [2, 3]], output_name: "all"}
```

#### `list_append`
Fields: `list`, `item`, `output_name`<br>
Appends `item` to `list` and stores the result.<br>
Example:<br>
```json5
{cmd: "list_append", list: [1, 2], item: 3, output_name: "all"}
```

#### `list_remove`
Fields: `list`, `item`, `output_name`<br>
Removes the first matching `item` from `list` if present.<br>
Example:<br>
```json5
{cmd: "list_remove", list: [1, 2, 2], item: 2, output_name: "rest"}
```

#### `list_index`
Fields: `list`, `index`, `output_name`<br>
1-based indexing; negative indices count from the end.<br>
Example:<br>
```json5
{cmd: "list_index", list: ["a", "b", "c"], index: -1, output_name: "last"}
```

#### `list_slice`
Fields: `list`, `from_index`, `to_index`, `output_name`<br>
1-based, right-inclusive slicing; indices may be math expressions.<br>
Example:<br>
```json5
{cmd: "list_slice", list: [1, 2, 3, 4], from_index: 2, to_index: 3, output_name: "mid"}
```

#### `user_input`
Fields: `prompt`, `output_name`<br>
Prompts the user; input is escaped before storing.<br>
Example:<br>
```json5
{cmd: "user_input", prompt: "Name? ", output_name: "name"}
```

#### `user_choice`
Fields: `list`, `description`, `output_name`<br>
Presents a list to the user and stores the chosen item.<br>
Example:<br>
```json5
{cmd: "user_choice", list: ["small", "large"], description: "Size", output_name: "size"}
```

#### `label`
Fields: `name`<br>
Defines a label for `goto` and `goto_map`.<br>
Example:<br>
```json5
{cmd: "label", name: "@start"}
```

#### `goto`
Fields: `name`<br>
Jumps to a label. Not supported inside `parallel_*` tasks.<br>
Example:<br>
```json5
{cmd: "goto", name: "@start"}
```

#### `goto_map`
Fields: `text`, `target_maps`<br>
Conditional goto. `target_maps` is a list of single-entry dicts mapping patterns (with `*` wildcards) to label names. Supports `NULL` key when interpolation fails. Not supported inside `parallel_*` tasks.<br>
Example:<br>
```json5
{cmd: "goto_map", text: "{user_input}", target_maps: [{"yes": "@ok"}, {"*": "@fallback"}]}
```

#### `replace_map`
Fields: `item`, `output_name`, `wildcard_maps`<br>
Optional: `repeat_until_done` (bool)<br>
Applies wildcard pattern replacements; supports `NULL` key for interpolation errors.<br>
Example:<br>
```json5
{cmd: "replace_map", item: "Age 41", output_name: "age", wildcard_maps: [{"Age *": "{1}"}]}
```

#### `for`
Fields: `name_list_map`, `tasks`<br>
Iterates lists in lockstep and runs `tasks` for each iteration. Lists must be the same length.<br>
Example:<br>
```json5
{cmd: "for", name_list_map: {"name": ["A", "B"]}, tasks: [{cmd: "print", text: "{name}\n"}]}
```

#### `serial`
Fields: `tasks`<br>
Runs nested tasks sequentially.<br>
Example:<br>
```json5
{cmd: "serial", tasks: [{cmd: "print", text: "A"}, {cmd: "print", text: "B"}]}
```

#### `parallel_wait`
Fields: `tasks`<br>
Runs tasks concurrently and waits for all to finish.<br>
Example:<br>
```json5
{cmd: "parallel_wait", tasks: [{cmd: "sleep", seconds: 1}, {cmd: "sleep", seconds: 1}]}
```

#### `parallel_race`
Fields: `tasks`<br>
Runs tasks concurrently and cancels the others once one finishes.<br>
Example:<br>
```json5
{cmd: "parallel_race", tasks: [{cmd: "sleep", seconds: 1}, {cmd: "sleep", seconds: 2}]}
```

#### `run_task`
Fields: `task_name`<br>
Runs a task from `program.tasks` by name. Extra fields are passed through.<br>
Example:<br>
```json5
{cmd: "run_task", task_name: "my_task"}
```

#### `delete`
Fields: `wildcards`<br>
Deletes inserts matching wildcard patterns.<br>
Example:<br>
```json5
{cmd: "delete", wildcards: ["tmp/*"]}
```

#### `delete_except`
Fields: `wildcards`<br>
Deletes all inserts except those matching wildcard patterns.<br>
Example:<br>
```json5
{cmd: "delete_except", wildcards: ["user/*"]}
```

#### `math`
Fields: `input`, `output_name`<br>
Evaluates a math expression and stores the result.<br>
Example:<br>
```json5
{cmd: "math", input: "3 + 4 * 2", output_name: "result"}
```

#### `chat`
Fields: `messages`, `output_name`<br>
Optional: `model` (required unless program-level `completion_args` supplies it)<br>
Other optional args: `n_outputs`, `start_str`, `stop_str`, `hide_start_str`, `hide_stop_str`, `shown`, `choices_list_name`, `choices_list`, `extra_body`, `max_completion_tokens`, `temperature`, `seed`, `stop`<br>
Example:<br>
```json5
{cmd: "chat", messages: [{role: "user", content: "Hi"}], output_name: "reply", model: "gpt-4o-mini"}
```

#### `generate`
Fields: `prompt`, `output_name`<br>
Optional: `model` (required unless program-level `completion_args` supplies it)<br>
Other optional args: `n_outputs`, `start_str`, `stop_str`, `hide_start_str`, `hide_stop_str`, `shown`, `num_ctx`, `repeat_last_n`, `repeat_penalty`, `temperature`, `seed`, `stop`, `num_predict`, `top_k`, `top_p`, `min_p`<br>
Example:<br>
```json5
{cmd: "generate", prompt: "Say hello", output_name: "reply", model: "llama3"}
```
