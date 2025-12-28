Interpolation Engine is a CLI tool to execute *programs* defined by JSON5 files.

**Why JSON5?** Valid *programs* are a subset of JSON5. JSON is unambiguous, easy to parse, fast to parse, and easy to write for experienced programmers. It is can express the nested structures that Interpolation Engine requires. I use JSON5 because I want comments, trailing commas, and optional quotes for keys.

## Command Reference

Each task in a program is a dict with a `cmd` key, e.g. `{"cmd": "print", "text": "Hello"}`. Fields shown below are required unless marked optional.

- `print`
  - Fields: `text`
  - Appends text to output and writes it to the screen.

- `clear`
  - Clears output and the screen.

- `sleep`
  - Fields: `seconds`
  - Sleeps for the given duration. Accepts numbers or a math expression string.

- `set`
  - Fields: `item`, `output_name`
  - Stores `item` under `output_name` in `state.inserts`.

- `unescape`
  - Fields: `item`, `output_name`
  - Like `set`, but unescapes `\{` and `\}` and re-interpolates.

- `show_inserts`
  - Shows the current `state.inserts`.

- `random_choice`
  - Fields: `list`, `output_name`
  - Picks a random element from `list`.

- `join_list`
  - Fields: `list`, `before`, `between`, `after`, `output_name`
  - Joins list items into a string with prefix/suffix.

- `list_concat`
  - Fields: `lists`, `output_name`
  - Concatenates a list of lists.

- `list_append`
  - Fields: `list`, `item`, `output_name`
  - Appends `item` to `list` and stores the result.

- `list_remove`
  - Fields: `list`, `item`, `output_name`
  - Removes the first matching `item` from `list` if present.

- `list_index`
  - Fields: `list`, `index`, `output_name`
  - 1-based indexing; negative indices count from the end.

- `list_slice`
  - Fields: `list`, `from_index`, `to_index`, `output_name`
  - 1-based, right-inclusive slicing; indices may be math expressions.

- `user_input`
  - Fields: `prompt`, `output_name`
  - Prompts the user; input is escaped before storing.

- `user_choice`
  - Fields: `list`, `description`, `output_name`
  - Presents a list to the user and stores the chosen item.

- `label`
  - Fields: `name`
  - Defines a label for `goto` and `goto_map`.

- `goto`
  - Fields: `name`
  - Jumps to a label. Not supported inside `parallel_*` tasks.

- `goto_map`
  - Fields: `text`, `target_maps`
  - Conditional goto. `target_maps` is a list of single-entry dicts mapping patterns (with `*` wildcards) to label names. Supports `NULL` key when interpolation fails. Not supported inside `parallel_*` tasks.

- `replace_map`
  - Fields: `item`, `output_name`, `wildcard_maps`
  - Optional: `repeat_until_done` (bool)
  - Applies wildcard pattern replacements; supports `NULL` key for interpolation errors.

- `for`
  - Fields: `name_list_map`, `tasks`
  - Iterates lists in lockstep and runs `tasks` for each iteration. Lists must be the same length.

- `serial`
  - Fields: `tasks`
  - Runs nested tasks sequentially.

- `parallel_wait`
  - Fields: `tasks`
  - Runs tasks concurrently and waits for all to finish.

- `parallel_race`
  - Fields: `tasks`
  - Runs tasks concurrently and cancels the others once one finishes.

- `run_task`
  - Fields: `task_name`
  - Runs a task from `program.tasks` by name. Extra fields are passed through.

- `delete`
  - Fields: `wildcards`
  - Deletes inserts matching wildcard patterns.

- `delete_except`
  - Fields: `wildcards`
  - Deletes all inserts except those matching wildcard patterns.

- `math`
  - Fields: `input`, `output_name`
  - Evaluates a math expression and stores the result.

- `chat`
  - Fields: `messages`, `output_name`
  - Optional: `model` (required unless program-level `completion_args` supplies it)
  - Other optional args: `n_outputs`, `start_str`, `stop_str`, `hide_start_str`, `hide_stop_str`, `shown`, `choices_list_name`, `choices_list`, `extra_body`, `max_completion_tokens`, `temperature`, `seed`, `stop`

- `generate`
  - Fields: `prompt`, `output_name`
  - Optional: `model` (required unless program-level `completion_args` supplies it)
  - Other optional args: `n_outputs`, `start_str`, `stop_str`, `hide_start_str`, `hide_stop_str`, `shown`, `num_ctx`, `repeat_last_n`, `repeat_penalty`, `temperature`, `seed`, `stop`, `num_predict`, `top_k`, `top_p`, `min_p`
