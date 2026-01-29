## Interpolation Engine

Interpolation Engine is a CLI tool to execute programs defined by JSON5 files.

## Installation
```
pip install interpolation-engine
```

## Usage
Start a program with
```
interpolation_engine my_program.json5
```
Press `escape` at any time to toggle the main menu. Opening the menu stops program execution. Closing the menu resumes execution. From them menu you can save and load runtime states. Saved states are stored in the program file.
Hold shift to select text.
When prompted for text input you can press ctrl-n to enter linebreaks.

Agent mode (for automated testing) waits for `user_input`/`user_choice` via files:
```
interpolation_engine --agent-mode examples/text_adventure.json5
```
When a prompt is reached, `/tmp/agent_output` is written as JSON with fields `type`, `output`, and (if applicable) `prompt`/`choices`. For `user_choice`, `choices` is an object whose keys are valid inputs (e.g. `"1"`, `"2"` or `"a"`, `"b"`), and values are the option strings. Write the selected key (or exact option text) to `/tmp/agent_input` to resume.

## Writing Programs

#### General Architecture

Interpolation Engine expects to be passed a JSON5 file. This file needs to have a certain structure. I will call JSON5 a file with this structure a `program`. Before reading on, check out the `examples` directory. Its often easier to learn from an example than from an explaination.      

The behavior of a program is defined by the `order` list, the elements of which are tasks.

There are 28 commands in Interpolation Engine, and you can think of a task as a function call to one of these commands.
Here is what a task look like `{cmd: 'print', text:'My name is {name}.'}` 
You can see that a value with the key `name` is being interpolated into the string. This is not unique to the print command, in fact **every string** in interpolation engine can use interpolations.
 ```
{cmd: 'user_input', prompt:'{question-{i}}', output_name:'{persona_name}/answer-{i}'}
```
Take a look at the `prompt` value. Here we have some kind of index `i` that we interpolate to get something like `question-3`. Around all of this are another set of interpolation parentheses, meaning that interpolation engine will look up the value saved under `question-3` and display this as the prompt to the user.

Now consider the `output_name` value. (Commands that produce an output require an `output_name` specifying the insert key under which it will be stored, like a variable name.) Here we see that the `output_name` is also being interpolated. Perhaps this program is asking the user to answer questions as various personas. The slash in the output name is not syntactically relevant, but I find it helpful to structure my insert keys hierarchically. E.g. in this example if you wanted to delete anything related to the persona `Benjamin` you could use
```
{cmd:'delete', wildcards:['Benjamin/*'}
```

The values that can be interpolated into strings are called `inserts`. The inserts a program can access are the inserts in `program['default_state']['inserts']` + the inserts you define at runtime + the inserts in `inserts-dir`, if that argument was passed.

The order starts at `program['default_state']['order_index']` and will execute one task after another,
incrementing the `order_index`.

The order of executed tasks is affected by `goto`, `goto_map`. The tasks `serial`, `parallel_wait` and `parallel_race` execute sub-tasks (see below).

Tasks are usually defined directly in the `program['order']`, but you can also define named tasks in  `program['named_tasks']`.
There their key is a name by which they can be executed like `{cmd:'run_task', task_name:'print_current_status'}`.

Using `serial`, a named task can be arbitrarily complex which is the closest thing Interpolation Engine has to factoring code into a function.

`program['order']` and  `program['named_tasks']` are static. The complete runtime information is contained in it's so called `state`. `default_state` is simply the state that gets loaded when Interpolation Engine executes a program from the beginning. You can save and load the current state using the main menu.


#### Interpolaton

The contents of `state['inserts']` are mappings from keys to values.
Values of type Int, String, and even List (by way of `''.join`) can be interpolated into strings.

E.g. to use

    {cmd: 'print', text:'My name is {name}.'}

your `state['inserts']` would need to look like this:

    inserts: {
        name: 'tom',
        ...
    }

If an inerpolation key is not defined in state['inserts'], it can be looked up as a file in
an inserts directory passed via `--inserts-dir`. This is a convenient way to define inserts globally,
for all programs.

Special Interpolation keys:
    - 'HH:MM': Current time as HH:MM.
    - 'HH:MM:SS': Will be populated with the current time.
    - 'ARG1': 'The first argument passed into the program, only defined if one was passed. `{` and `}` will be escaped.
    - 'ARG2': 'The second argument passed into the program, only defined if one was passed. `{` and `}` will be escaped.
    - 'ARG{n}': 'The n-th argument passed into the program, only defined if one was passed. `{` and `}` will be escaped.


#### Escaping
The text enclosed in interpolation start and stop strings '{' and '}' will always be eagerly interpolated.
To escape this, use the escape string '//'. Unlike in other programming languages, these will not be 
automatically un-escaped. This allows you to safely handle them without interpolating undefined variables.
Use unescape to turn nested structures with escapes into their unescaped counterparts. This will also
realize every interpolation.

#### Output
When the program terminates without error, the last output will be printed to stdout. To not prevent this, clear the screen with 'clear' before exiting.

---

Note that the state exists only in the python runtime's memory, and `program['default_state']` will not be
updated. The user does have the option to save states to disk using the main menu.

Pressing escape at any time will gracefully abort the current task and toggle the main menu
where he can save his state to disk, or load the state from disk.

States are stored in right in the program definition at `program['save_states']`. Currently I allow
up to ten save slots with keys from '1' to '10'. The object saved at e.g. `program['save_states']['1']` is
simply the current state plus a string label that the user has to enter.

Because comments and custom indentation is useful for writing and reading the program's `order`, saving is done
by editing the program as a string instead of using json5.dump.

#### Indexing
Indices in the program are 1-based with right-left-inclusive slicing and -1, -2, ... denoting the last, penultimate, ... indices.


**Why JSON5?** Valid programs are a subset of JSON5. JSON is unambiguous, easy to parse, fast to parse, and easy to write for experienced programmers. It is can express the nested structures that Interpolation Engine requires. I use JSON5 because I want comments, trailing commas, and optional quotes for keys.

This is a list of all valid Interpolation Engine commands.

#### `print`
Fields: `text`<br>
Prints text to the user. Does not add a linebreak.<br>
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

#### `await_insert`
Fields: `name`<br>
Blocks until an insert with the given name exists.<br>
Example:<br>
```json5
{cmd: "await_insert", name: "user_input"}
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
Evaluates a mathematical expression. Result must be an integer. Useful for list index manipulation, counters and advanced control flow.<br>
Supports `+ - * / %` and parentheses; expressions are interpolated before evaluation.<br>
Functions: `length(name)`, `min(list_or_csv)`, `max(list_or_csv)`, `round(expr)`, `sign(expr)`.<br>
Example:<br>
```json5
{cmd: "math", input: "max(1,2,3) + length(items)", output_name: "result"}
```

#### `chat`
Fields: `messages`, `output_name`, `model`<br>
Optional: `n_outputs`, `start_str`, `stop_str`, `hide_start_str`, `hide_stop_str`, `shown`, `choices_list_name`, `choices_list`, `extra_body`, `max_completion_tokens`, `temperature`, `seed`, `stop`, `api_url`, `api_key`<br>
`chat` fields are joined with `program['completion_args']`. `chat` requires access to an OpenAI-API compatible endpoint. The default values for `api_url` and `api_key` are `http://localhost:8080` and `unused`, which assume that you have a llama.cpp server running locally. If you want to pass on generation parameters that are not supported by the OpenAI-API, use `extra_body`: `extra_body: {dry_base: 1.75}`
Example:<br>
```json5
{cmd: "chat", messages: [{role: "user", content: "Hi"}], output_name: "reply", model: "gpt-4o-mini"}
```
