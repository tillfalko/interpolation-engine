from copy import deepcopy
from .filter import inverted_filter, filter
from glob import glob
from hashlib import md5
from pydantic import BaseModel
from signal import SIGINT
import argparse
import asyncio # for parallel generation
import json # used only for dumping pydantic schema in structured generation
import json5
from openai import AsyncOpenAI
import os
import random # for random.choice
import re
import sys
from datetime import datetime # for the 'HH:MM' special insertkey
from typing import Literal

from prompt_toolkit import PromptSession, print_formatted_text, prompt # prompt function is used instead of `input` the user enters text.
from prompt_toolkit.application import Application
from prompt_toolkit.filters import Condition
from prompt_toolkit.history import InMemoryHistory, FileHistory
from prompt_toolkit.key_binding import KeyBindings
from prompt_toolkit.layout import ConditionalContainer, Layout, HSplit, ScrollablePane, Window, ScrollOffsets
from prompt_toolkit.buffer import Buffer
from prompt_toolkit.document import Document
from prompt_toolkit.layout.dimension import Dimension
from prompt_toolkit.layout.controls import BufferControl, FormattedTextControl
from prompt_toolkit.styles import Style
from prompt_toolkit.widgets import TextArea, Label, SearchToolbar
from prompt_toolkit.data_structures import Point
from time import time # deleteme


error_style = Style.from_dict({
    '': 'red', # This also changes the text the user types.
})

insert_start='{'
insert_stop='}'
escape = '\\' # Use this to escape '{' and '}'.
inserts_dir = None
AGENT_OUTPUT_PATH = "/tmp/agent_output"
AGENT_INPUT_PATH = "/tmp/agent_input"

class InputOutputManager:
    agent_mode = False
    _instance = None

    def __new__(cls):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
            cls._instance._init_once()
        return cls._instance

    def _init_once(self):
        self.agent_mode = self.__class__.agent_mode
        if self.agent_mode:
            self.prompt_history = InMemoryHistory()
            self.show_prompt = False
            self.show_input_info = False
            self.prompt_text = ''
            self.output_buffer = None
            self.output_text = ''
            self.output = None
            self.input_info = None
            self.prompt = None
            self.kb = None
            self.app = None
            self._input_future = None
            self._input_lock = asyncio.Lock()
            self._app_task = None
            return

        global prompt_history_path
        self.prompt_history = FileHistory(prompt_history_path) if prompt_history_path else InMemoryHistory()

        self.show_prompt = False
        self.show_input_info = False
        self.prompt_text = '' # needs to be an instance variable

        self.output_buffer = Buffer()

        self.output = Window(
            content=BufferControl(
                buffer = self.output_buffer
            ),
            #style='bg:red',
            wrap_lines=False,
            always_hide_cursor=True)

        self.input_info = TextArea(
            focusable=False,
            wrap_lines=True,
            #height=Dimension(weight=9),
            #dont_extend_height=False,
            style="class:input-field",
        )

        #self.search =SearchToolbar() 
        self.prompt = TextArea(
            # No fixed height: let it size to content (incl. wrapping)
            height=None,
            dont_extend_height=True,
            history=self.prompt_history,
            #search_field=self.search,
            wrap_lines=True,
            # prompt can be a function. Here it is necessary because otherwise we couldn't update the prompt.
            #prompt=lambda: self.prompt_text,
            get_line_prefix = lambda i_line, n_wrap: self.prompt_text if i_line == 0 else ' '*len(self.prompt_text),
            style="class:input-field",
            multiline=True,
            scrollbar=False,              # optional: scroll if content exceeds available space
            read_only=Condition(lambda: not self.show_prompt),
        )


        self.kb = KeyBindings()

        @self.kb.add("c-d")
        def _(event):
            global killme
            killme = True
            t = menu_state['async_task']
            if t:
                t.cancel()

        @self.kb.add("escape", eager=True)
        def _(event):
            toggle_menu()

        style = Style.from_dict({"input-field": "fg:yellow"})

        self.app = Application(
            layout=Layout(
                HSplit([
                    self.output,
                    ConditionalContainer(self.input_info, filter=Condition(lambda: self.show_input_info)),
                    ConditionalContainer(self.prompt, filter=Condition(lambda: self.show_prompt)),
                    #ConditionalContainer(self.search, filter=Condition(lambda: self.show_prompt))
                ]),
                focused_element=self.prompt
            ),
            key_bindings=self.kb,
            style=style,
            mouse_support=True,
            full_screen=True,
        )
        self.app.ttimeoutlen=0  # make 'escape' shortcut instant

        self._input_future = None
        self._input_lock = asyncio.Lock()
        self._app_task = None

    async def start(self):
        """Start the prompt_toolkit app in the background."""
        if self.agent_mode:
            return
        if self._app_task is None:
            self._app_task = asyncio.create_task(self.app.run_async())


    async def stop(self):
        """Exit the UI."""
        if self.agent_mode:
            return
        if self._app_task and not self._app_task.done():
            self.app.exit()
            await self._app_task

    async def clear(self):
        if self.agent_mode:
            self.output_text = ''
            return
        self.output_buffer.reset()
        # self.output is only as big as it needs to be. When empty, the input_info is at the top
        # of the screen instead of the bottom. To prevent this, always have some empty lines.
        await self.write('\n'*100) 

        self.app.invalidate()

    async def write(self, text: str):
        if self.agent_mode:
            self.output_text += text
            return
        ri = self.output.render_info
        if ri is None: # ri is None when you try to write before InputOutputManager.start()
            new_text = self.output_buffer.text + text
            self.output_buffer.set_document(
                Document(new_text, cursor_position=len(new_text)),
                bypass_readonly=True,
            )
            self.app.invalidate()
            return

        old_doc = self.output_buffer.document
        follow_output = old_doc.is_cursor_at_the_end

        # Use the end-of-buffer column for wrapping, not the cursor position.
        last_line = old_doc.text.rsplit('\n', 1)[-1]
        free_space = ri.window_width -len(last_line) 
        to_print = ''

        while text != '':
            to_print += text[0]
            free_space = ri.window_width if text[0] == '\n' else free_space - 1
            text = text[1:]

            if free_space < 1:
                text = '\n' + text

        new_text = old_doc.text + to_print
        new_cursor = len(new_text) if follow_output else old_doc.cursor_position
        self.output_buffer.set_document(
            Document(new_text, cursor_position=new_cursor),
            bypass_readonly=True,
        )

        self.app.invalidate()

    async def user_input(self, prompt: str, default :str = '') -> str:
        """
        Ask the user for input with a multi-line prompt.
        - Supports Shift+Enter to insert newlines.
        - Enter submits.
        """
        if self.agent_mode:
            try:
                os.remove(AGENT_INPUT_PATH)
            except FileNotFoundError:
                pass
            payload = {
                "type": "user_input",
                "output": self.output_text,
                "prompt": prompt,
            }
            with open(AGENT_OUTPUT_PATH, "w") as f:
                f.write(json.dumps(payload, ensure_ascii=True, indent=2))
            while True:
                if os.path.exists(AGENT_INPUT_PATH):
                    with open(AGENT_INPUT_PATH, "r") as f:
                        data = f.read()
                    try:
                        os.remove(AGENT_INPUT_PATH)
                    except FileNotFoundError:
                        pass
                    return data.rstrip('\n')
                await asyncio.sleep(0.1)

        if '\n' in prompt:
            outline_prompt, inline_prompt = prompt.rsplit('\n', maxsplit=1)
        else:
            outline_prompt, inline_prompt = '', prompt

        #assert False, f"outline_prompt: {repr(outline_prompt)}\ninline_prompt: {repr(inline_prompt)}"

        async with self._input_lock:
            # Render the multi-line prompt *inside* the input field
            self.show_prompt = True
            self.show_input_info = outline_prompt != ''
            self.input_info.buffer.text = outline_prompt
            self.prompt_text = inline_prompt
            self.prompt.buffer.insert_text(default)
            # Move cursor to the end
            self.app.invalidate()

            self._input_future = asyncio.get_event_loop().create_future()

            # insert newline
            @self.kb.add("c-n")
            def _(event):
                self.prompt.buffer.insert_text("\n")

            # Enter: submit
            @self.kb.add("enter")
            def _(event):
                text = self.prompt.text
                self.prompt_history.append_string(text)
                if self._input_future and not self._input_future.done():
                    self._input_future.set_result(text)
                self.kb.remove('enter')
                #self.kb.remove("escape","enter")

            try:
                result = await self._input_future
            except asyncio.CancelledError as e:
                raise e
            finally:
                # reset
                self.input_info.buffer.text = ''
                self.prompt_text = ''
                self.prompt.buffer.reset()
                self.show_prompt = False
                self.show_input_info = False

            self.app.invalidate()
            return result


    async def select_index(self, options: list, description : str = None) -> str:
        """
        Present a list of options inside the input area and wait for a keypress selection.
        #The input area's height dynamically adjusts to fit all options, accounting for wrapping.
        """
        if self.agent_mode:
            if len(options) <= 9:
                keys = [str(i) for i in range(1, len(options) + 1)]
            else:
                keys = [chr(ord('a') + i) for i in range(len(options))]
            choice_map = {k: i for i, k in enumerate(keys)}
            payload = {
                "type": "user_choice",
                "output": self.output_text,
                "prompt": description,
                "choices": {k: options[i] for k, i in choice_map.items()},
            }
            try:
                os.remove(AGENT_INPUT_PATH)
            except FileNotFoundError:
                pass
            with open(AGENT_OUTPUT_PATH, "w") as f:
                f.write(json.dumps(payload, ensure_ascii=True, indent=2))
            while True:
                if os.path.exists(AGENT_INPUT_PATH):
                    with open(AGENT_INPUT_PATH, "r") as f:
                        raw = f.read()
                    try:
                        os.remove(AGENT_INPUT_PATH)
                    except FileNotFoundError:
                        pass
                    text = raw.strip()
                    if text in choice_map:
                        return choice_map[text]
                    if text in options:
                        return options.index(text)
                    raise Exception(
                        f"Invalid agent choice '{raw}'. Expected one of: {', '.join(choice_map.keys())}."
                    )
                await asyncio.sleep(0.1)

        all_keys = ('1','2','3','4','5','6','7','8','9','9','a','b','c','d','e','f','g','h','i','j','k','l','m','n','o','p','q','r','s','t','u','v','w','x','y','z')

        if len(options) > len(all_keys):
            raise Exception(f"SELECT_INDEX ERROR: Got {len(options)} keys. That is too much.")

        async with self._input_lock:
            # Build option text
            option_lines = []
            for i, option in enumerate(options):
                if i >= len(all_keys):
                    continue
                option_lines.append(f"({all_keys[i]}) {option}")

            self.prompt.height=0
            self.input_info.buffer.text = "\n".join([description]+option_lines if description else option_lines)
            self.show_input_info = True
            self.app.invalidate()

            # Determine keys
            keys = [str(i) for i in range(1, len(options) + 1)] if len(options) <= 9 else [
                chr(ord('a') + i) for i in range(len(options))
            ]

            self._input_future = asyncio.get_event_loop().create_future()

            def make_handler(index):
                def handler(event):
                    if self._input_future and not self._input_future.done():
                        self._input_future.set_result(index)

                return handler

            for i, key in enumerate(keys):
                self.kb.add(key)(make_handler(i))

            try:
                result = await self._input_future
            except asyncio.CancelledError as e:
                # Reset
                for key in keys[:len(options)]:
                    self.kb.remove(key)
                self.prompt.height = None
                self.input_info.buffer.text = ''
                self.prompt_text = ''
                self.show_input_info = False
                self.app.invalidate()
                raise e

            # Reset
            for key in keys[:len(options)]:
                self.kb.remove(key)
            self.prompt.height = None
            self.input_info.buffer.text = ''
            self.prompt_text = ''
            self.show_input_info = False
            self.app.invalidate()

            return result



def str_preview(s):
    s = repr(s)
    if len(s) <= 45:
        return s
    else:
        return s[:20] + '[...]' + s[-20:]

class InterpolationException(Exception):
    # Caught in goto_map to trigger the 'NULL' key.
    pass

def get_interpdata(inserts, insertkey: str):
    match insertkey:
        # special interpolation values
        case 'HH:MM': 
            return datetime.now().strftime("%H:%M")
        case 'HH:MM:SS': 
            return datetime.now().strftime("%H:%M:%S")
        case str() as a if a.startswith("ARG") and (num := a[3:]).isdigit():
            # Caputure e.g. ARG1

            if insertkey not in inserts:
                raise InterpolationException(
                    f"Argument interpolation key '{insertkey}' is used, but the user passed less than {num} program arguments.")
            return inserts[insertkey]


        case '': 
            raise InterpolationException(f"Tried to interpolate empty string ''.")

        case insertkey:
            if insertkey in inserts:
                return inserts[insertkey]
            if inserts_dir:
                try:
                    try:
                        with open(os.path.join(inserts_dir, f"{insertkey}.json5")) as f:
                            return recursive_escape(json5.loads(f.read()))
                    except FileNotFoundError:
                        with open(os.path.join(inserts_dir, insertkey)) as f:
                            return recursive_escape(f.read().strip())
                except FileNotFoundError:
                    pass
            missing_detail = " in interpdata"
            if inserts_dir:
                missing_detail += f" or inserts directory '{inserts_dir}'"
            raise InterpolationException((
                f"Could not find variable '{insertkey}'{missing_detail}. "
                f"Available interpolation data keys are {list(inserts.keys())}."))


def set_interpdata(inserts, insertkey: str, insertvalue: str):
    inserts[insertkey] = insertvalue

def delete_interpdata(inserts, insertkey: str):
    if insertkey in inserts:
        del inserts[insertkey]

def get_simple_insertkey(content):
    # returns an insertkey or None if content is not a simple interpolation
    # '{{name}/description}' is a simple insertkey, and may be an int, a list, or a dict.
    # '{name}/{description}' is not a simple insertkey, and will always be a string.
    if type(content) != str:
        return None

    depth = 0
    for i,c in enumerate(content):
        if c == insert_stop:
            depth -= 1

        if (depth == 0) != (i == 0 or i == len(content)-1):
            return None

        if c == insert_start:
            depth += 1

    return content[len(insert_start):-len(insert_stop)]

def interpolate_inserts(inserts, content : str) :
    # return type can be anything
    # Escaped interpolation characters will un-escaped after the interpolation phase.
    escaped_insert_start = escape+insert_start
    escaped_insert_stop  = escape+insert_stop
    replaced_escaped_insert_start = '.〠'
    replaced_escaped_insert_stop  = '〠.'

    content = (content
        .replace(escaped_insert_start, replaced_escaped_insert_start)
        .replace(escaped_insert_stop, replaced_escaped_insert_stop)
    )

    if (insertkey := get_simple_insertkey(content)):
        if (sub_insertkey := get_simple_insertkey(insertkey)):
            return get_interpdata(inserts, interpolate_inserts(inserts, insert_start + sub_insertkey + insert_stop))
        else:
            return get_interpdata(inserts, interpolate_inserts(inserts,insertkey))

    while content.find(insert_start) != -1:
        n_insert_starts = content.count(insert_start) - content.count(escape+insert_start)
        n_insert_stops = content.count(insert_stop) - content.count(escape+insert_stop)
        assert n_insert_starts == n_insert_stops, f"Error: The following content has {n_insert_starts} '{insert_start}' and {n_insert_stops} '{insert_stop}':\n\n\"\"\"{content}\n\"\"\""
        outer_from = content.rfind(insert_start)
        inner_to   = content.find(insert_stop, outer_from+len(insert_start))
        if outer_from == -1 or inner_to == -1: break
        inner_from = outer_from + len(insert_start)
        outer_to   = inner_to   + len(insert_stop)
        insertkey = (content[inner_from:inner_to]
            .replace(replaced_escaped_insert_start, escape+insert_start)
            .replace(replaced_escaped_insert_stop, escape+insert_stop)
        )
        insertvalue = get_interpdata(inserts, insertkey)
        assert type(insertvalue) in (str, int, list), f"Error: trying to interpolate variable '{insertkey}' of type {type(insertvalue)} into a string."
        content = content[:outer_from] + str(insertvalue) + content[outer_to:]
        content = (content
            .replace(escaped_insert_start, replaced_escaped_insert_start)
            .replace(escaped_insert_stop, replaced_escaped_insert_stop)
        )

    content = (content
        .replace(replaced_escaped_insert_start, escaped_insert_start)
        .replace(replaced_escaped_insert_stop, escaped_insert_stop)
    )
    return content


def interpolate_messages_inserts(inserts, messages):
    new_messages = []
    for message in messages:
        role = message['role']
        content = message['content']
        content = interpolate_inserts(inserts, content)
        new_messages.append({'role':role,'content':content.strip()})
    return new_messages

def is_wildcard_match(wildcard_s, s):
    pattern = '^' + re.escape(wildcard_s.replace('*','〠')).replace('〠', '(.*)') + '$'
    return bool(re.match(pattern, s, re.DOTALL)) # DOTALL means that '.*' will also match newlines.

def get_wildcard_matches(wildcard_s, s):
    pattern = '^' + re.escape(wildcard_s.replace('*','〠')).replace('〠', '(.*)') + '$'
    X =  re.findall(pattern, s, re.DOTALL) # DOTALL means that '.*' will also match newlines.
    result = []
    for x in X:
        if type(x) == tuple:
            result.extend(x)
        else:
            result.append(x)

    return result


# caching
client = last_api_url = last_api_key = None

async def chat(
        messages,
        completion_args,
        start_str,
        stop_str,
        hide_start_str,
        hide_stop_str,
        n_outputs,
        shown,
        choices_list,
        api_url,
        api_key,
        extra_body,
    ):
    """
    Generates output live while printing and filtering out e.g. <output>.
    Can extract multiple outputs when `n_outputs > 1`. If `shown==True`, the outputs will be enumerated from 1.
    Args:
        Required:
            messages (list[dict]) : List of messages in OpenAI api format ('role','content')
            completion_args (dict) : args in the format required by ollama.chat to set e.g. 'model'.
        Optional:
            start_str (str) : What delimits the start of an output. E.g. '<output>'.
            stop_str (str) : What delimits the end of an output. E.g. '</output>'.
            hide_start_str (str) : E.g. '<think>'. Text between hide_start_str and hide_stop_str will not be printed.
            hide_stop_str (str) : E.g. '</think>'
            n_outputs (int) : The amount of outputs expected. Can be -1 to be unlimited.
            shown (bool) : Whether to print what is being generated to stdout.
            choices_list (list[str]) : A list of choices that the model will be restricted to pick from.
    Returns:
        if n_outputs == 1:
            output (str) : The generated output with start_str and stop_str removed.
                           If choices_list was passed, the output is guaranteed to be an element.
        else:
            outputs (list[str]) : A list of generated outputs with start_str and stop_str removed.
    """
    
    assert bool(start_str) == bool(stop_str), "You can either set both start_str and stop_str or none. Right now you have only set one of them."
    if choices_list != None:
        assert start_str == stop_str == "", "Filtering is not supported when using choices."
        assert n_outputs == 1, "Multiple outputs not supported when using choices."


    global client, last_api_url, last_api_key # recreating the client takes 100ms-200ms, caching
    if last_api_url != api_url or last_api_key != api_key:
        client = AsyncOpenAI(base_url=api_url, api_key=api_key)
        last_api_url = api_url
        last_api_key = api_key


    # will be shown should generation run out of context length.
    async def out_of_context_message():
        log_string('Ran out of context length, generation stopped short.', title='WARNING')
        await InputOutputManager().select_index([], 'Generation exceeded context length! Instead of crashing, this message is being shown so that you can save and try to increase your context length before loading. Loading this save will restart the generation.')


    raw = ""
    visual_output = ""
    print(
        f"🛈  Starting generation with these completion_args: {completion_args}",
        file=log_sink,
    )

    response = None
    ran_out_of_context = False
    try:

        if choices_list == None:

            # Set stop if n == 1.
            #if n_outputs == 1 and stop_str:
                #completion_args['stop'] = completion_args.get('stop',[]) + [stop_str]

            response = await client.chat.completions.create(
                messages=messages,
                stream=True,
                extra_body=extra_body,
                **completion_args)

            hide_filter = inverted_filter(hide_start_str, hide_stop_str)
            extract_outputs,outputs = filter(start_str, stop_str, enumerate_outputs = n_outputs > 1)
            async for comp in response:
                # last completion looks like this
                # ChatCompletionChunk(id='chatcmpl-Ymca30RQi7uVQdcYJeIt4uIIQGmfVhRX', choices=[Choice(delta=ChoiceDelta(content=None, function_call=None, refusal=None, role=None, tool_calls=None), finish_reason='stop', index=0, logprobs=None)], created=1765671497, model='...', object='chat.completion.chunk', service_tier=None, system_fingerprint='b7342-2fbe3b7', usage=None, timings={'cache_n': 49, 'prompt_n': 1, 'prompt_ms': 0.571, 'prompt_per_token_ms': 0.571, 'prompt_per_second': 1751.3134851138354, 'predicted_n': 14, 'predicted_ms': 958.716, 'predicted_per_token_ms': 68.47971428571428, 'predicted_per_second': 14.602864664822532})

                chunk = comp.choices[0]
                if chunk.finish_reason=='length':
                    ran_out_of_context = True
                delta = chunk.delta.content
                if not delta is None:
                    raw += delta
                    fragment = extract_outputs(delta)
                    if shown:
                        visual_fragment = hide_filter(fragment)
                        await InputOutputManager().write(visual_fragment)
                        visual_output += visual_fragment
            

        elif choices_list:

            class Choice(BaseModel):
                # Literal wants a tuple not a list.
                choice: Literal[tuple(choices_list)]

            schema = json.dumps(Choice.model_json_schema())
            schema_prompt = f"Respond only with a valid JSON object conforming to this schema: {schema}. Do not add any additional text."

            updated_messages = messages + [{'role': 'user', 'content': schema_prompt}]

            response =  await client.chat.completions.create(
                messages=updated_messages,
                stream=True,
                response_format={'type':'json_schema', 'json_schema':schema},
                extra_body=extra_body,
                **completion_args)

            async for comp in response:
                chunk = comp.choices[0]
                if chunk.finish_reason=='length':
                    ran_out_of_context = True
                delta = chunk.delta.content
                if not delta is None:
                    raw += delta

                    if shown:
                        await InputOutputManager().write(delta)
                        visual_output += delta


            outputs = [Choice.model_validate_json(raw).choice]

    except BaseException as e:
        
        if response:
            # properly cancel generation
            await response.close()

        # Log Output even if interrupted.
        log_messages( messages + [{'role':'assistant','content':raw}] )

        if 'exceeds the available context size' in str(e) or 'Context size has been exceeded' in str(e):
            await out_of_context_message()

        raise e

    if ran_out_of_context:
        await out_of_context_message()


    if shown:
        await InputOutputManager().write('\n')
        visual_output += '\n'

    log_messages( messages + [{'role':'assistant','content':raw}] )



    return [o.strip() for o in outputs], visual_output


math_legal_terminals = set(" .0123456789+-*/%")

def math_safe_eval(s):
    assert set(s) <= math_legal_terminals
    s = s.replace('^','**')
    return eval(s)

def math_length(inserts : dict, inner : str):
    _list = get_interpdata(inserts, inner)
    assert type(_list) == list, f"'math_length' was called on '{inner}', which is of type {type(_list)}, but 'length' expects a list."
    return len(_list)

def math_min(inserts : dict, inner : str):
    # min accepts either a list name or a list.
    if set(inner) <= (math_legal_terminals|{','}):
        return min(math_safe_eval(x) for x in inner.split(','))
    else:
        _list = get_interpdata(inserts, inner)
        assert type(_list) == list, f"'math_min' was called on '{inner}', which is of type {type(_list)}, but 'min' expects either an enumeration of ints or a list."
        return min(_list)

def math_max(inserts : dict, inner : str):
    # max accepts either a list name or a list.
    if set(inner) <= (math_legal_terminals|{','}):
        return max(math_safe_eval(x) for x in inner.split(','))
    else:
        _list = get_interpdata(inserts, inner)
        assert type(_list) == list, f"'math_max' was called on '{inner}', which is of type {type(_list)}, but 'max' expects either an enumeration of ints or a list."
        return max(_list)

def math_round(inserts : dict, inner : str):
    return round(math_safe_eval(inner))

def math_sign(inserts : dict, inner : str):
    value = math_safe_eval(inner)
    if value > 0:
        return 1
    elif value < 0:
        return -1
    else:
        return 0


# All implemented math functions. Each of these must take the inputs (inserts: dict, inner: str).
math_functions = {
    'length': math_length,
    'min'   : math_min,
    'max'   : math_max,
    'round' : math_round,
    'sign'  : math_sign,
}
    
def eval_math(inserts, math_input : str) -> int:
    print(f"    Math:    {math_input}", file=log_sink)
    math_input = interpolate_inserts(inserts, math_input)

    width = len(math_input)
    
    assert math_input.count('(') == math_input.count(')'), f"Math Errror: Illegal parentheses in \"{math_input}\"."

    operator_chars = set("+-*/^%")
    word_splitting_chars = set(" ()+-*/^%")

    while math_input.find('(') != -1:
        print(f"    Math: => {math_input.ljust(width)}", file=log_sink, end='')
        outer_from = math_input.rfind('(')
        inner_to   = math_input.find(')', outer_from+len('('))
        if outer_from == -1 or inner_to == -1: break
        inner_from = outer_from + len('(')
        outer_to   = inner_to   + len(')')
        inner = math_input[inner_from:inner_to]
        if math_input[outer_from-1] in word_splitting_chars:
            # In this case, the parentheses do not belong to a function and can be evaluated.
            subresult = math_safe_eval(inner)
            print(f"  //  ({inner}) = {subresult}", file=log_sink)
        else:
            # Interpret last word as function name.
            function_name = ''.join([c if c not in word_splitting_chars else ' ' for c in math_input[:outer_from]]).split()[-1]
            outer_from -= len(function_name)
            if function_name in math_functions:
                subresult = math_functions[function_name](inserts, inner)
                print(f"  //  {function_name}({inner}) = {subresult}", file=log_sink)
            else:
                assert False, f"In expression '{math_input}', unprocessable function name '{function_name}' was encountered."

        math_input = math_input[:outer_from] + str(subresult) + math_input[outer_to:]

    print(f"    Math: => {math_input}", file=log_sink)

    illegal_chars = set(math_input) - math_legal_terminals
    assert not illegal_chars, (
        f"Mathematical expression '{math_input}' contains illegal characters: {', '.join(repr(c) for c in sorted(illegal_chars))}. "
        "Perhaps you meant to interpolate an insert.")
    result = eval(math_input)
    print(f"    Math: => {result}", file=log_sink)
    result_int = round(result)
    print(f"    Math: => {result_int}", file=log_sink)
    if result != 0:
        assert abs((result_int - result)/result) < 0.0001, f"Got reult {result}, but currently results are restricted to be integers."

    return result_int



def splice_key_into_json5(content: str, key: str, new_value: dict, n_indent = 4):
    """
    Replaces the dictionary value of a key in a JSON5 file, preserving comments and formatting.

    Args:
        file_path: Path to the JSON5 file.
        key: The key whose value (an object) is to be replaced.
        new_value: The new dictionary to set as the value.
    """
    # 1. Find the key followed by a colon and an opening brace.
    # This is more robust than a simple string search.
    match = re.search(f"(['\"]?{key}['\"]?)\\s*:\\s*{{", content)
    if not match:
        print(f"Key '{key}' not found or it's not an object.", file=log_sink)
        return

    # 2. Find the matching closing brace.
    # We start searching from the opening brace found by the regex.
    start_pos = match.end() - 1
    brace_level = 1
    end_pos = -1
    for i in range(start_pos + 1, len(content)):
        # This simple scan assumes braces in strings/comments are not an issue,
        # as per the problem description's simplification.
        if content[i] == '{':
            brace_level += 1
        elif content[i] == '}':
            brace_level -= 1
        
        if brace_level == 0:
            end_pos = i
            break
    
    assert end_pos != -1, "Error: Could not find matching closing brace."

    # 3. Determine the indentation of the key's line.
    # This will be used to format the new dictionary content nicely.
    line_start_pos = content.rfind('\n', 0, match.start()) + 1
    key_indent = content[line_start_pos:match.start()]
    # 4. Serialize the new dictionary to a JSON5 string.
    # We assume a standard sub-indent of 2 spaces.
    new_value_dump = json5.dumps(new_value, indent=n_indent, quote_keys=True)
    # 5. Extract the inner content from the dumped string (lines between braces).
    inner_lines = new_value_dump.splitlines()[1:-1]
    # 6. Re-indent the new content to fit the original file's structure.
    formatted_inner_lines = [key_indent + line for line in inner_lines]
    # 7. Construct the final replacement, adding newlines for clean formatting.
    replacement = f"\n" + "\n".join(formatted_inner_lines) + f"\n{key_indent}"
    # 8. Splice the new content into the original file content.
    new_content = content[:start_pos + 1] + replacement + content[end_pos:]

    return new_content


def log_messages(messages):
    print("\n----------------------------MESSAGES--------------------------", file=log_sink)
    print('\n\n'.join([f"{m['role'].upper()}\n{m['content']}" for m in messages]), file=log_sink)
    print("\n--------------------------------------------------------------", file=log_sink)
def log_string(s, title=''):
    print(f"\n----------------------------{title}----------------------------", file=log_sink)
    print(s, file=log_sink)



def validate_program(program):
    # Validates program and adds 'traceback_label' to each task in order.

    assert 'default_state' in program, f"Key 'state' not in program. Does it follow the new format?"
    assert 'save_states' in program and type(program['save_states']) == dict
    #assert 'output' in program['default_state'] and type(program['default_state']['output']) == str, f"default_task needs 'output' str, only has keys {program.keys()}"
    assert 'named_tasks' in program and type(program['named_tasks']) == dict, f"program needs 'named_tasks' object for named tasks"
    assert 'inserts' in program['default_state'] and type(program['default_state']['inserts']) == dict

    # It is not possible to know beforehand what content inserts will have
    # so I cannot know for certain if a task tries to interpolate 
    # an unset key. But I can check if it tries to interpolate a key
    # that never ever will be defined in program, nor is a member of the inserts dir.
    all_insertkeys_ever_available = set(program['default_state']['inserts'].keys())
    # add special inserts
    all_insertkeys_ever_available |= {'HH:MM', 'HH:MM:SS'}
    if inserts_dir:
        insert_files = glob(os.path.join(inserts_dir, '*'))
        insert_keys = []
        for path in insert_files:
            filename = os.path.basename(path)
            if filename.endswith('.json5'):
                filename = filename[:-len('.json5')]
            insert_keys.append(filename)
        all_insertkeys_ever_available |= set(insert_keys)
    tasks_to_check = program['order'].copy() + list(program['named_tasks'].values())
    for i,task in enumerate(tasks_to_check):
        assert 'line' in task, f"This task does not have a 'line' key: {task}"
        task['traceback_label'] = f"{task['cmd']}-{task['line']}"
    unexplored_tasks = tasks_to_check.copy()
    labels_seen = ['CONTINUE'] # CONTINUE is a reserved label name that does not need to be set with a label.

    while len(unexplored_tasks) > 0:
        task = unexplored_tasks.pop()
        insertkeys_defined = set()
        insertkeys_used = {insertkey for v in task.values() if (insertkey := get_simple_insertkey(v))}

        if 'output_name' in task:
            insertkeys_defined |= {task['output_name']}
        if task['cmd'] == 'for':
            insertkeys_defined |= set(task['name_list_map'].keys())
        if item := task.get('item', False):
            if 'cmd' in item:
                item['traceback_label'] = task['traceback_label'] + f"/{item['cmd']}-{item['line']}"
                unexplored_tasks.append(item)
                tasks_to_check.append(item)
        if 'tasks' in task:
            subtasks = task['tasks']
            if get_simple_insertkey(subtasks):
                continue # in this case subtasks is a string with simple interpolation and not a list of tasks
            for subtask in subtasks:
                if get_simple_insertkey(subtask):
                    continue # in this case subtasks is a string with simple interpolation and not a task
                subtask['traceback_label'] = task['traceback_label'] + f"/{subtask['cmd']}-{subtask['line']}"
            unexplored_tasks.extend([t for t in subtasks if not get_simple_insertkey(t)])
            tasks_to_check.extend([t for t in subtasks if not get_simple_insertkey(t)])
        if task['cmd'] == 'label':
            # Make sure labels are unique
            assert not task['name'] in labels_seen, f"{task['traceback_label']}: Label '{task['name']}' is not unique."
            labels_seen.append(task['name'])

        # process insertkeys_defined and insertkeys_used
        # example: 'transcript/{enum}' ∈ insertkeys_used => insertkeys_defined += 'transcript/*', insertkeys_used += 'enum'
        while True:
            clean = True
            for outer_insertkey in insertkeys_defined.copy():
                outer_from = outer_insertkey.rfind(insert_start)
                inner_to   = outer_insertkey.find(insert_stop, outer_from+len(insert_start))
                if outer_from == -1 or inner_to == -1: continue
                clean = False
                inner_from = outer_from + len(insert_start)
                outer_to   = inner_to   + len(insert_stop)
                insertkey = outer_insertkey[inner_from:inner_to]
                insertkeys_used.add(insertkey)
                insertkeys_defined.remove(outer_insertkey)
                insertkeys_defined.add( outer_insertkey[:outer_from] + '*' + outer_insertkey[outer_to:] )
            if clean: break

    
        # Common error: a task like {cmd:'write', text:'{log}\n{entry}', output_name:'log'}
        # without ever defining 'log'. Since 'log' is the output_name, if I didn't substract insertkeys
        # used, this would not be an error.
        all_insertkeys_ever_available |= insertkeys_defined - insertkeys_used


    order_item_delim = '|。' # To figure out the index of the order item containing an invalid key, we
    # just need to count how many of these are in the text leading up to it. Use weird unicode so it's near impossible
    # that the actual program contains this.

    texts_delim = '|、' # If a key contains this, it means that it was formed from one text ending in insert_start
    # and another starting with insert_stop, so even if the key is invalid we should not raise an exception because
    # the key does not exist in the actual program. Use weird unicode so it's near impossible that the actual program
    # contains this.

    any_marker = '<〠>' # This shows that some valid key has been interpolated into the content at this spot.
    # So if "tom/location" is in all_insertkeys_ever_available, and "<any>/location" is our insertkey,
    # we will not raise an exception. Use weird unicode so it's near impossible that the actual program contains this.

    def to_string(val):
        if type(val) == str:
            return val
        elif type(val) in (int, float, bool): # E.g. n_outputs, temperature, shown
            return str(val)
        elif type(val) == list:
            return texts_delim.join(to_string(x) for x in val)
        elif type(val) == dict:
            return texts_delim.join(to_string(k)+texts_delim+to_string(v) for (k,v) in val.items())
        else:
            raise Exception(f"Encountered value {val} of type {type(val)} in to_string.")

    content = order_item_delim + order_item_delim.join([texts_delim.join(to_string(val) for val in order_item.values()) for order_item in program['order']]) + order_item_delim

    escaped_insert_start = escape+insert_start
    escaped_insert_stop  = escape+insert_stop
    replaced_escaped_insert_start = '.〠'
    replaced_escaped_insert_stop  = '〠.'

    content = (content
        .replace(escaped_insert_start, replaced_escaped_insert_start)
        .replace(escaped_insert_stop, replaced_escaped_insert_stop)
    )

    # Index is correct, because content starts with order_item_delim (first order_s will be '').
    for order_index, order_s in enumerate(content.split(order_item_delim)):
        for field in order_s.split(texts_delim):
            assert field.count(insert_start) == field.count(insert_stop), (
                f"Order Index {order_index}: The following content has an uneven number of '{insert_start}' and '{insert_stop}':\n\n\"\"\"{field}\"\"\""
            )

    while content.find(insert_start) != -1:
        # Parse keys from the inside out.
        outer_from = content.rfind(insert_start)
        inner_to   = content.find(insert_stop, outer_from+len(insert_start))

        found_stop = inner_to != -1 and not texts_delim in content[inner_to:outer_from]

        order_index = content[:outer_from].count(order_item_delim)
        assert found_stop, (
            f"Order Index {order_index}: Malformed insert key,  singular '{insert_start}'")

        inner_from = outer_from + len(insert_start)
        outer_to   = inner_to   + len(insert_stop)
        insertkey = content[inner_from:inner_to]

        #pattern =  '(.*)'.join( re.escape(part) for part in insertkey.split(any_marker) )
        pattern =  '*'.join( re.escape(part) for part in insertkey.split(any_marker) )

        is_possible_key = 0 < sum([
            #1 if re.match(pattern, key) or (insert_start in key and insert_stop in key) else 0 # HACK! keys from output_name:'description_{enumerator}' could match 'description_A'
            1 if (is_wildcard_match(pattern, key) or is_wildcard_match(key, pattern)) else 0
            for key in all_insertkeys_ever_available
        ])

        current_order_item = content[
            content[:outer_from].rindex(order_item_delim) + len(order_item_delim):
            outer_to + content[outer_to:].index(order_item_delim)]

        # numeric special keys may be possible in replace_map.
        if insertkey.replace(any_marker,'').isnumeric() and 'replace_map' in current_order_item:
            is_possible_key = True

        # ARG1, ARG2 etc require special handling for pretty error message.
        if insertkey.startswith("ARG") and (num := insertkey[3:]).isdigit():
            assert int(num) > 0, f"Order Index {order_index}: Argument interpolation keys must be greater than 0. '{insertkey}' is not valid."
            # skip checking if the program uses ARG{n} that the user didn't pass.
            # downside: catch less bugs during validation
            # upside: all programs to change their behavior by testing with goto_map how many args were passed.
            is_possible_key = True # assertions passed

        pretty_key = insertkey.replace(any_marker, '<Any>')
        errortext = (
            f"Order Index {order_index}: Insert key '{pretty_key}' will never be defined for any value of <Any>."
            if any_marker in insertkey else
            f"Order Index {order_index}: Insert key '{pretty_key}' will never be defined."
        )
        assert is_possible_key, errortext

        # Replace valid interpolated values with any_marker.
        content = content[:outer_from] + any_marker + content[outer_to:]

    # Same algorithm but as above but to check values ending in '_name'.
    def is_possible_key(s):
        assert s.count(insert_start) == s.count(insert_stop), f"Malformed interpolation: {s}"

        # If there are no further interpolations in s, check if it could match one of the insertkeys.
        if s.count(insert_start) == 0:
            pattern =  '(.*)'.join( re.escape(part) for part in s.split(any_marker) )
            return 0 < sum([
                1 if re.match(pattern, key) else 0
                for key in all_insertkeys_ever_available
            ])

        # If not, recursively check if it is a valid key.
        if s.count(insert_start) > 0:
            outer_from = s.rfind(insert_start)
            inner_to   = s.find(insert_stop, outer_from+len(insert_start))
            inner_from = outer_from + len(insert_start)
            outer_to   = inner_to   + len(insert_stop)
            insertkey = s[inner_from:inner_to]
            s = s[:outer_from] + any_marker + s[outer_to:]
            return is_possible_key(insertkey) and is_possible_key(s)


    def validate_task(task):
        for k,v in task.items():
            if (insertkey := get_simple_insertkey(content)):
                assert is_possible_key(insertkey), f"{task['traceback_label']}: trying to interpolate '{insertkey}', which will never be defined."

        def assert_types(field_name, legal_types):
            if get_simple_insertkey(task[field_name]) and str not in legal_types:
                # field is simple interpolation and may be anything
                legal_types.append(str)
            t = type(task[field_name])
            assert t in legal_types, f"{task['traceback_label']}: field '{field_name}' has value '{t}', but must be one of {legal_types}."
            
        match task:

            case {'cmd':'list_join', 'list': _, 'before': _, 'between': _, 'after': _, 'output_name': _}:
                assert_types('list', [list])
                assert_types('before', [str])
                assert_types('between', [str])
                assert_types('after', [str])
                assert_types('output_name', [str])

            case {'cmd':'list_concat', 'lists':_, 'output_name': _}:
                assert_types('lists', [list])
                assert_types('output_name', [str])

            case {'cmd':'list_append', 'list':_, 'item':_, 'output_name': _}:
                assert_types('list', [list])
                assert_types('output_name', [str])

            case {'cmd':'list_remove', 'list':_, 'item':_, 'output_name': _}:
                assert_types('list', [list])
                assert_types('output_name', [str])

            case {'cmd':'list_index', 'list':_, 'index':_, 'output_name': _}:
                assert_types('list', [list])
                assert_types('index', [int, str]) # str for math  input
                assert_types('output_name', [str])

            case {'cmd':'list_slice', 'list':_, 'from_index':_, 'to_index':_, 'output_name': _}:
                assert_types('list', [list])
                assert_types('from_index', [int, str]) # str for math  input
                assert_types('to_index', [int, str]) # str for math  input
                assert_types('output_name', [str])

            case {'cmd':'user_choice', 'list': _, 'output_name': _, 'description':_}:
                assert_types('list', [list])
                assert_types('description', [str])
                assert_types('output_name', [str])

            case {'cmd':'user_input', 'prompt':_, 'output_name': _}:
                assert_types('prompt', [str])
                assert_types('output_name', [str])

            case {'cmd':'await_insert', 'name': _}:
                assert_types('name', [str])
                if not get_simple_insertkey(task['name']):
                    assert is_possible_key(task['name']), (
                        f"{task['traceback_label']}: await_insert name '{task['name']}' will never be defined."
                    )

            case {'cmd':'run_task', 'task_name':task_name, **extra_args}:
                assert_types('task_name', [str])
                assert task_name in program['named_tasks'], f"{task['traceback_label']}: Task '{task_name}' is used at but never defined."

            case {'cmd':'parallel_race', 'tasks':_}:
                assert_types('tasks', [list])

            case {'cmd':'parallel_wait', 'tasks':_}:
                assert_types('tasks', [list])

            case {'cmd':'serial', 'tasks':_}:
                assert_types('tasks', [list])

            case {'cmd':'label', 'name':_}:
                assert_types('name', [str])

            case {'cmd':'set', 'item': _, 'output_name': _}:
                assert_types('output_name', [str])

            case {'cmd':'unescape', 'item': _, 'output_name':_}:
                assert_types('output_name', [str])

            case {'cmd':'print', 'text':_}:
                assert_types('text', [str])

            case {'cmd':'sleep', 'seconds':_}:
                assert_types('seconds', [float, int])

            case {'cmd':'clear'}:
                pass

            case {'cmd':'goto', 'name':target}:
                assert_types('name', [str])
                assert target in labels_seen, f"{task['traceback_label']}: Goto is pointing at '{target}', which is not defined.\n\nAvailable labels: {labels_seen}"
                assert not task['traceback_label'].rsplit('/',maxsplit=1)[-1].startswith('parallel'), f"{task['traceback_label']}: goto is not supported in parallel."

            case {'cmd':'goto_map', 'text': value_text, 'target_maps':target_maps}:
                assert_types('text', [str])
                assert_types('target_maps', [list])

                for x in target_maps:
                    assert type(x) == dict and len(x) == 1, f"{task['traceback_label']}: Elements of target_maps have to be dicts with one key-value-pair. The item {x} does not match."

                target_keys = [next(iter(d.keys())) for d in target_maps]
                target_values = [next(iter(d.values())) for d in target_maps]

                no_interpolation_used = 0 == sum(insert_start in x for x in [value_text]+target_keys)
                no_wildcard = 0 == sum('*' in k for k in target_keys)
                if no_interpolation_used and no_wildcard:
                    assert value_text in target_keys, f"{task['traceback_label']}: value_text ({value_text}) is neither interpolated nor in target keys, and because there is no wildcard, this goto_map will fail."
                    

                for target in target_values:
                    if insert_start not in target and target not in labels_seen:
                        raise Exception(f"{task['traceback_label']}: goto_map is pointing at '{target}', which is not defined.")
                assert not task['traceback_label'].rsplit('/',maxsplit=1)[-1].startswith('parallel'), f"{task['traceback_label']}: goto_map is not supported in parallel."

            case {'cmd':'replace_map', 'item': _, 'output_name':_, 'wildcard_maps':_, **extra_args}:
                assert_types('wildcard_maps', [list])
                assert_types('output_name', [str])

            case {'cmd':'for', 'name_list_map':_, 'tasks':_}:
                assert_types('name_list_map', [dict])
                assert_types('tasks', [list])

            case {'cmd':'show_inserts'}:
                pass

            case {'cmd':'random_choice', 'output_name':output_name, 'list': _}:
                assert_types('list', [list])
                assert_types('output_name', [str])

            case {'cmd':'delete', 'wildcards': wildcards}:
                assert_types('wildcards', [list])
                if type(wildcards) == list:
                    for wildcard in wildcards:
                        if get_simple_insertkey(wildcard):
                            # wilcards is interpolated at runtime, can't be checked here
                            continue
                        never_defined = True
                        for k in all_insertkeys_ever_available:
                            if is_wildcard_match(wildcard, k):
                                never_defined = False
                                break
                        assert not never_defined, f"{task['traceback_label']}: you want to delete '{wildcard}', but this will never be defined."

            case {'cmd':'delete_except', 'wildcards': name_list}:
                assert_types('wildcards', [list])
                if type(name_list) == list:
                    for wildcard in name_list:
                        #if get_simple_insertkey(wildcard):
                        #    # wilcards is interpolated at runtime, can't be checked here
                        #    continue
                        never_defined = True
                        for k in all_insertkeys_ever_available:
                            if is_wildcard_match(wildcard, k):
                                never_defined = False
                                break
                        assert not never_defined, f"{task['traceback_label']}: you want to delete '{wildcard}', but this will never be defined."

            case {'cmd':'math', 'input': math_input, 'output_name':_}:
                assert_types('input', [str])
                assert_types('output_name', [str])
                assert math_input.count('(') == math_input.count(')'), f"{task['traceback_label']}: Illegal parentheses in \"{math_input}\"."

            case {'cmd':'chat', **args}:
                
                arg_set = set(args) # Set only considers the keys of a dict.
                required_args = {'messages', 'output_name'}

                permitted_args = {
                    'messages', 'output_name', 'n_outputs', 'start_str', 'stop_str', 'hide_start_str', 'hide_stop_str', 'shown', 'choices_list_name', 'choices_list', 'traceback_label', 'line', 'model',
                    # the rest are opanai api options https://platform.openai.com/docs/api-reference/chat/create
                    'extra_body', 'max_completion_tokens', 'temperature', 'seed', 'stop' 
                }
                if 'completion_args' not in program:
                    required_args |= {'model'}

                assert ('start_str' in arg_set) == ('stop_str' in arg_set), f"{task['traceback_label']}: You can either set both start_str and stop_str or none. Right now you have only set one of them."
                assert arg_set <= permitted_args, f"{task['traceback_label']}: chat has illegal arguments {arg_set - permitted_args}."
                assert arg_set >= required_args, f"{task['traceback_label']}: chat is missing required arguments {required_args - arg_set}."
                assert type(args['messages']) in (str, list)

                if type(args['messages']) == list:
                    for i,message in enumerate(args['messages']):
                        if not get_simple_insertkey(message):
                            assert type(message) == dict
                            assert 'role' in message, f"{task['traceback_label']}: 'Message number {i+1} does not have 'role'."
                            assert 'content' in message, f"{task['traceback_label']}: 'Message number {i+1} does not have 'role'."
                            assert message['role'] in ('user', 'system', 'assistant'), f"{task['traceback_label']}: 'Message number {i+1} has unknown role '{message['role']}'."


            case somethingelse:
                raise Exception(f"{task['traceback_label']}: Found unexpected task: {somethingelse}.")


    for task in tasks_to_check:
        validate_task(task)

def task_preview(task):
    return ", ".join([f"{k}={str_preview(v)}" for k,v in task.items() if k not in ('traceback_label',)])

def recursive_unescape(x):
    if type(x) == str:
        return (x
            .replace(escape+insert_start, insert_start)
            .replace(escape+insert_stop, insert_stop))

    elif type(x) == list:
        return [recursive_unescape(xx) for xx in x]
    elif type(x) == dict:
        return {recursive_unescape(xk):recursive_unescape(xv) for xk,xv in x.items()}
    else:
        return x

def recursive_escape(x):
    if type(x) == str:
        return (x
            .replace(insert_start, escape+insert_start)
            .replace(insert_stop, escape+insert_stop))

    elif type(x) == list:
        return [recursive_escape(xx) for xx in x]
    elif type(x) == dict:
        return {recursive_escape(xk):recursive_escape(xv) for xk,xv in x.items()}
    else:
        return x

def recursive_interpolate(inserts, x):
    if get_simple_insertkey(x):
        return recursive_interpolate(inserts, interpolate_inserts(inserts, x))
    elif type(x) == str:
        return interpolate_inserts(inserts, x)
    elif type(x) == list:
        return [recursive_interpolate(inserts, xx) for xx in x]
    elif type(x) == dict:
        # HANDLE CERTAIN TASKS SPECIAL
        if 'cmd' in x and x['cmd'] == 'goto_map':
            # goto_map has to do its own interpolation because it can catch interpolation errors.
            return x
        
        elif 'cmd' in x and x['cmd'] == 'replace_map':
            # replace_map has to do its own interpolation because it can catch interpolation errors.
            return x

        elif 'cmd' in x and x['cmd'] in ('for', 'serial', 'parallel_wait', 'parallel_race'):
            # only interpolate simple inserts into tasks, but don't recursively interpolate task contents.
            # otherwise, every insertkey anywhere in the tasks would have to be defined NOW.
            # but it should be legal to define an insertkey in a previous task within the tasks.
            x = deepcopy(x)
            if (insertkey := get_simple_insertkey(x['tasks'])):
                x['tasks'] = get_interpdata(inserts, insertkey)
            for i_subtask in range(len(x['tasks'])):
                if (insertkey := get_simple_insertkey(x['tasks'][i_subtask])):
                    x['tasks'][i_subtask] = get_interpdata(inserts, insertkey)
            return x

        else:
            return {recursive_interpolate(inserts, xk):recursive_interpolate(inserts, xv) for xk,xv in x.items()}
    else:
        return x


async def execute_task(state, task, completion_args, named_tasks, runtime_label : str):
    inserts = state['inserts']
    print(f"🛈  Order Item {task['traceback_label']}:  {task_preview(task)}", file=log_sink, flush=True)

    task = recursive_interpolate(inserts, task)

    match task:

        case {'cmd':'list_join', 'list': _list, 'before': before, 'between': between, 'after': after, 'output_name': output_name}:
            set_interpdata(inserts, output_name, before + between.join(_list) + after)

        case {'cmd':'list_concat', 'lists':lists, 'output_name': output_name}:
            set_interpdata(inserts, output_name, sum(lists, start =[]))

        case {'cmd':'list_append', 'list':_list, 'item':item, 'output_name': output_name}:
            set_interpdata(inserts, output_name, _list + [item])

        case {'cmd':'list_remove', 'list':_list, 'item':item, 'output_name': output_name}:
            _list = deepcopy(_list)
            try:
                _list.remove(item)
            except ValueError:
                # Don't crash on redundant list_remove.
                pass
            set_interpdata(inserts, output_name, _list)

        case {'cmd':'list_index', 'list':_list, 'index':index, 'output_name': output_name}:

            # Unlike python, order indexing is 1-based.
            def py_index(index):
                index = int(index) if type(index) == str else index
                if type(index) == int and index > 0:
                    return index-1
                elif type(index) == int and index < 0:
                    return len(_list) + index
                else:
                    raise Exception(f"Program lists cannot be indexed with '{index}'. Programs are 1-indexed.")

            set_interpdata(inserts, output_name, _list[py_index(index)])

        case {'cmd':'list_slice', 'list':_list, 'from_index':from_index, 'to_index':to_index, 'output_name': output_name}:
            # And unlike python, order indexing is left-right-inclusive.
            from_index = eval_math(inserts, from_index) if type(from_index) == str else from_index
            to_index = eval_math(inserts, to_index) if type(to_index) == str else to_index

            # Unlike python, order indexing is 1-based.
            def py_index(index, right=False):
                index = int(index) if type(index) == str else index
                if type(index) == int and index > 0:
                    return index-1
                elif type(index) == int and index < 0:
                    return len(_list) + index
                elif type(index) == int and right and index == 0:
                    return 0
                elif type(index) == int  and index == 0:
                    raise Exception(f"Lower index of slice cannot be 0. Programs are 1-indexed.")
                elif type(index) == int and index < 0:
                    raise Exception(f"Program lists cannot be indexed with '{index}'.")

            set_interpdata(inserts, output_name, _list[py_index(from_index):py_index(to_index, right=True)+1])

        case {'cmd':'user_choice', 'list': _list, 'output_name': output_name, 'description':description}:
            choice_index = await InputOutputManager().select_index(_list, description = description)
            choice = _list[choice_index]
            print(f"🛈  User selected {str_preview(choice)}.", file=log_sink)
            set_interpdata(inserts, output_name, choice)

        case {'cmd':'user_input', 'prompt':inputtext, 'output_name': output_name}:
            userinput = await InputOutputManager().user_input(prompt=inputtext)
            userinput = (userinput
                .replace(insert_start, escape+insert_start)
                .replace(insert_stop, escape+insert_stop))
            print(f"🛈  User entered {str_preview(userinput)}.", file=log_sink)
            set_interpdata(inserts, output_name, userinput)

        case {'cmd':'await_insert', 'name':name}:
            while name not in inserts:
                await asyncio.sleep(0.05)

        case {'cmd':'run_task', 'task_name':task_name, **extra_args}:
    
            subtask = named_tasks[task_name]
            return await execute_task(state, subtask, completion_args, named_tasks, f"{runtime_label}/{subtask['traceback_label']}")

        case {'cmd':'parallel_wait', 'tasks':tasks}:

            # tasks may have been added at runtime, in which case they would not
            # have received a traceback label.
            for i,subtask in enumerate(tasks):
                # If subtask has traceback_label, use traceback_label.
                # if subtask has line number, use that, else just enumerate
                subtask['traceback_label'] = subtask.get('traceback_label', f"({subtask['cmd']}-{subtask.get('line', i+1)})")

            await asyncio.gather(
                *(execute_task(state, t, completion_args, named_tasks, f"{runtime_label}/{t['traceback_label']}") for t in tasks),
            )

        case {'cmd':'parallel_race', 'tasks':tasks}:

            # tasks may have been added at runtime, in which case they would not
            # have received a traceback label.
            for i,subtask in enumerate(tasks):
                # If subtask has traceback_label, use traceback_label.
                # if subtask has line number, use that, else just enumerate
                subtask['traceback_label'] = subtask.get('traceback_label', f"({subtask['cmd']}-{subtask.get('line', i+1)})")

            pending = {asyncio.create_task(execute_task(state, t, completion_args, named_tasks, f"{runtime_label}/{t['traceback_label']}")) for t in tasks}
            try:
                done, pending = await asyncio.wait(
                    pending,
                    return_when=asyncio.FIRST_COMPLETED,
                )
            except asyncio.CancelledError:
                for p in pending:
                    p.cancel()
                await asyncio.gather(*pending, return_exceptions=True)
                raise
            else:
                for p in pending:
                    p.cancel()
                # if the first task finished, the others are done too and must have
                # their order order_indices removed too. This affects serial. serial usually removes its own order index, but when
                # interrupted this is not possible as an interuption can come from the 
                # main menu in which case the order index should not be removed.
                for k in tuple(state.keys()):
                    if k.startswith(f"order_index/{runtime_label}"):
                        del state[k]
                await asyncio.gather(*pending, return_exceptions=True)
                first_task = done.pop()
                await first_task
            
        case {'cmd':'serial', 'tasks':tasks}:

            # tasks may have been added at runtime, in which case they would not
            # have received a traceback label.
            for i,subtask in enumerate(tasks):
                # If subtask has traceback_label, use traceback_label.
                # if subtask has line number, use that, else just enumerate
                subtask['traceback_label'] = subtask.get('traceback_label', f"({subtask['cmd']}-{subtask.get('line', i+1)})")

            sub_index_label = f"order_index/{runtime_label}"
            state[sub_index_label] = state.get(sub_index_label, 1)
            while state[sub_index_label] <= len(tasks): # order_index is 1-based.
                subtask = tasks[state[sub_index_label] - 1]
                result = await execute_task(state, subtask, completion_args, named_tasks, f"{runtime_label}/{subtask['traceback_label']}") # -1 because order_index is 1-based.
                match result:
                    case None:
                        state[sub_index_label] += 1
                    case {'goto_target': goto_target}:
                        state[sub_index_label] = 2 + min( # +1 for 1-indexing and +1 to go past the label
                            i for i in range(len(tasks))
                            if tasks[i]['cmd'] == 'label' and tasks[i]['name'] == goto_target)
                    case somethingelse:
                        raise Exception(f"{tasks[state[sub_index_label]]['traceback_label']}: Task {tasks[state[sub_index_label]]} returned unexpected value: {somethingelse}.")

            del state[sub_index_label]

        case {'cmd':'label', 'name':_}:
            pass # Label will be searched for when I encounter 'goto'.

        case {'cmd':'set', 'item': item, 'output_name': output_name}:
            set_interpdata(inserts, output_name, item)

        case {'cmd':'unescape', 'item': item, 'output_name': output_name}:

            # works just like 'set' but also unescapes strings.


            item = recursive_unescape(item)
            item = recursive_interpolate(inserts, item)
            set_interpdata(inserts, output_name, item)

        case {'cmd':'print', 'text':text}:
            # Remove escaping for insert start and insert stop for printing.
            # Otherwise it would not be possible to print the text '{notaninsert}'.
            text = text.replace(escape+insert_start,insert_start).replace(escape+insert_stop,insert_stop)
            text = str(text) # may be an int or list
            state['output'] += text
            await InputOutputManager().write(text)

        case {'cmd':'sleep', 'seconds':seconds}:
            seconds = eval_math(inserts, seconds) if type(seconds) == str else seconds
            await asyncio.sleep(seconds)

        case {'cmd':'clear'}:
            state['output'] = ''
            await InputOutputManager().clear()

        case {'cmd':'goto', 'name':target}:
            if target != 'CONTINUE':
                return {'goto_target': target}

        case {'cmd':'goto_map', 'text': value_text, 'target_maps':target_maps}:
            try:
                value_text = str(interpolate_inserts(inserts, value_text)) # we need to cast to str because for simple insertkeys may yield ints lists etc
                interp_error = False
            except InterpolationException:
                interp_error = True

            target_keys = [str(interpolate_inserts(inserts, next(iter(d.keys())))) for d in target_maps]
            target_values = [str(interpolate_inserts(inserts, next(iter(d.values())))) for d in target_maps]
            

            if interp_error:
                assert 'NULL' in target_keys, f"Order Index {task['traceback_label']}: value text '{value_text}' could not be resolved but 'NULL' is not a key in target_maps."
                target = target_values[ target_keys.index('NULL') ]
                print(f"🛈  goto_map value could not be resolved ('NULL'), proceeding to {target}", file=log_sink)
            else:
                matching_targets = [target for key,target in zip(target_keys, target_values) if is_wildcard_match(key, value_text)]
                assert len(matching_targets) > 0, f"Order Index {task['traceback_label']}: goto_map has no matches for '{value_text}'."
                target = matching_targets[0] # select first match. This is why we use a list of dicts for order.
                print(f"🛈  goto_map value is {value_text=}, proceeding to {target}", file=log_sink)

            if target != 'CONTINUE':
                return {'goto_target': target}

        case {'cmd':'replace_map', 'item': item, 'output_name':output_name, 'wildcard_maps':wildcard_maps, **extra_args}:

            output_name = interpolate_inserts(inserts, output_name)
            repeat_until_done = extra_args.get('repeat_until_done', False)

            def replace_str(text):
                last = current = text
                print(f"replace_map:\n    {str_preview(current)} \\\\ Interpolate", file=log_sink)
                while True:
                    current = str(interpolate_inserts(inserts, current)) # this may raise InterpolationException, will be caught
                    print(f"    => {str_preview(current)} \\\\ Find match", file=log_sink)

                    for d in wildcard_maps:
                        k = next(iter(d.keys()))
                        v = next(iter(d.values()))
                        k = str(interpolate_inserts(inserts, k))

                        if is_wildcard_match(k, current):
                            matches = get_wildcard_matches(k, current)
                            extra_inserts = {str(i+1):capture for i,capture in enumerate(matches)}
                            print(f"        Key: {str_preview(k)}\n        Matches: {str_preview(matches)}", file=log_sink)
                            current = str(interpolate_inserts(inserts | extra_inserts, v))
                            break

                    print(f"    => {str_preview(current)}", file=log_sink)

                    if last == current or not repeat_until_done:
                        return current

                    last = current

            def recursive_replace(x):
                if (insertkey := get_simple_insertkey(x)):
                    if (subkey := get_simple_insertkey(insertkey)):
                        return recursive_replace(insert_start + get_interpdata(inserts, subkey) + insert_stop)
                    else:
                        return recursive_replace(get_interpdata(inserts, insertkey))
                elif type(x) == str:
                    return replace_str(x)
                elif type(x) == list:
                    return [recursive_replace(xx) for xx in x]
                elif type(x) == dict:
                    return {recursive_replace(xk):recursive_replace(xv) for xk,xv in x.items()}
                else:
                    return x

            no_value = []
            value_if_error = ([next(iter(d.values())) for d in wildcard_maps if next(iter(d.keys())) == 'NULL'] + [no_value])[0]

            try:
                item = recursive_replace(item)
            except InterpolationException as e:

                if value_if_error is no_value:
                    assert False, f"{task['traceback_label']}: replace_map encountered an interpolation error without 'NULL' key: {repr(e)}"
                else:
                    print(f"        InterpolationError                     Matches: {str_preview(value_if_error)}", file=log_sink)
                    set_interpdata(inserts, output_name, value_if_error )
                    return

            set_interpdata(inserts, output_name, item )
               





        case {'cmd':'for', 'name_list_map':name_list_map, 'tasks':tasks}:

            # tasks may have been added at runtime, in which case they would not
            # have received a traceback label.
            for i,subtask in enumerate(tasks):
                # If subtask has traceback_label, use traceback_label.
                # if subtask has line number, use that, else just enumerate
                subtask['traceback_label'] = subtask.get('traceback_label', f"({subtask['cmd']}-{subtask.get('line', i+1)})")

            lists = [recursive_interpolate(inserts, name) for name in name_list_map.values()]
            item_names = [recursive_interpolate(inserts, _list) for _list in name_list_map.keys()]

            list_lengths = [len(l) for l in lists]

            assert len(set(list_lengths)) == 1, (
                f"Lists have differing lengths {list_lengths}. Maybe zipping lists of "
                f"unequal lengths should be supported, but currently it is not in order to catch "
                f"logical errors.")

            lists_length = list_lengths[0]

            # SERIAL
            counter_label = f"order_index/{runtime_label}/counter"

            state[counter_label]   = state.get(counter_label, 1)

            while state[counter_label] <= lists_length: # order_index is 1-based.
                print(f"🛈  For loop starting iteration {state[counter_label]}", file=log_sink)
                for item_name, _list in zip(item_names, lists):
                    print(f"🛈  For loop: {item_name} set to {_list[state[counter_label] -1]}", file=log_sink)
                    set_interpdata(inserts, item_name, _list[state[counter_label] - 1])

                sub_index_label = f"order_index/{runtime_label}"
                state[sub_index_label] = state.get(sub_index_label, 1)

                while state[sub_index_label] <= len(tasks): # order_index is 1-based.
                    subtask = tasks[state[sub_index_label] - 1]
                    result = await execute_task(state, subtask, completion_args, named_tasks, f"{runtime_label}/{subtask['traceback_label']}") # -1 because order_index is 1-based.
                    match result:
                        case None:
                            state[sub_index_label] += 1
                        case {'goto_target': goto_target}:
                            state[sub_index_label] = 2 + min( # +1 for 1-indexing and +1 to go past the label
                                i for i in range(len(tasks))
                                if tasks[i]['cmd'] == 'label' and tasks[i]['name'] == goto_target)
                        case somethingelse:
                            raise Exception(f"{tasks[state[sub_index_label]]['traceback_label']}: Task {tasks[state[sub_index_label]]} returned unexpected value: {somethingelse}.")

                state[counter_label] += 1
                del state[sub_index_label]

            del state[counter_label]



        case {'cmd':'show_inserts'}:
            await InputOutputManager().select_index(options=['Dismiss'], description=json5.dumps(inserts,indent=4) + '\n')

        case {'cmd':'random_choice', 'output_name':output_name, 'list': _list}:
            random_choice = random.choice(_list)
            print(f"🛈  Random choice resulted in '{str_preview(random_choice)}'.", file=log_sink)
            set_interpdata(inserts, output_name, random_choice)

        case {'cmd':'delete', 'wildcards': wildcards}:

            # Cast as tuple because the number of keys may change during iteration.
            for k in tuple(inserts.keys()):
                should_delete = False
                for wildcard in wildcards:
                    if is_wildcard_match(str(wildcard), str(k)):
                        should_delete = True
                        break

                if should_delete:
                    print(f"🛈  delete: '{k}'", file=log_sink)
                    delete_interpdata(inserts, k)

        case {'cmd':'delete_except', 'wildcards': wildcards}:
            # Cast as tuple because the number of keys may change during iteration.
            for k in tuple(inserts.keys()):
                should_delete = True
                for wildcard in wildcards:
                    if is_wildcard_match(str(wildcard), str(k)):
                        should_delete = False
                        break

                if should_delete:
                    print(f"🛈  delete: '{k}'", file=log_sink)
                    delete_interpdata(inserts, k)

        case {'cmd':'math', 'input': math_input, 'output_name':output_name}:

            result_int = eval_math(inserts, math_input)
            set_interpdata(inserts, output_name, result_int)

        case {'cmd':'chat', 'messages':messages, 'output_name': output_name, **other_args}:

            completion_args = deepcopy(completion_args)
            other_args['extra_body'] = other_args.get('extra_body',{})
            other_args['extra_body'].update(completion_args.pop('extra_body',{}))
            completion_args.update(other_args)

            start_str             = completion_args.pop('start_str', '')
            stop_str              = completion_args.pop('stop_str', '')
            hide_start_str        = completion_args.pop('hide_start_str', '')
            hide_stop_str         = completion_args.pop('hide_stop_str', '')
            n_outputs             = completion_args.pop('n_outputs', 1)
            shown                 = completion_args.pop('shown', True)
            choices_list          = completion_args.pop('choices_list', None)
            extra_body            = completion_args.pop('extra_body', {})
            api_url               = completion_args.pop('api_url', 'http://localhost:8080') # default for llama.cpp 
            api_key               = completion_args.pop('api_key', 'unused') # required by openai even if not used
            _                     = completion_args.pop('traceback_label', None)
            _                     = completion_args.pop('line', None)

            n_outputs = int(n_outputs) if type(n_outputs) is str and n_outputs.isnumeric() else n_outputs
            shown = True if shown == 'true' else shown
            shown = False if shown == 'false' else shown
            assert type(shown) == bool


            # max_completion_tokens is the proper argument going forward according to the openai
            # api, but it's broken in llama.cpp so we use max_tokens
            # https://github.com/abetlen/llama-cpp-python/issues/1907
            if 'max_completion_tokens' in completion_args:
                completion_args['max_tokens'] = completion_args.pop('max_completion_tokens')

            while True:
                # Output may be a list, but visual_output is always a str and it 
                # does not include text filtered with hide_start_str.
                output, visual_output = await chat(
                    messages=messages,
                    completion_args=completion_args,
                    start_str=start_str,
                    stop_str=stop_str,
                    hide_start_str=hide_start_str,
                    hide_stop_str=hide_stop_str,
                    n_outputs=n_outputs,
                    shown=shown,
                    choices_list=choices_list,
                    api_url=api_url,
                    api_key=api_key,
                    extra_body=extra_body
                )

                if len(output) < n_outputs:
                    await InputOutputManager().write(f"\n(Expected {n_outputs} outputs, got {len(output)}. Retrying.)\n")
                    await asyncio.sleep(2)
                    continue
                elif len(output) == 1:
                    set_interpdata(inserts, output_name, output[0])
                else:
                    set_interpdata(inserts, output_name, output)

                state['output'] += visual_output
                break

        
        case somethingelse:
            raise Exception(f"Got unprocessable task: {somethingelse}.\nThis should have been caught during validation and is a bug!")


async def main_menu(program, state, completion_args, named_tasks, filepath):
    status = '' # e.g. 'Loaded state successfully!'
    while True:
        leave = True # Thank god python does not have evil gotos.
        options = ["Save State", "Load State", "Reload and Restart", "Quit"]
        choice = options[await InputOutputManager().select_index(options, description = f"\n{status}")]
        print(f"🛈 user picked '{choice}'", file=log_sink)
        match choice:
            case "Save State":

                slot_states = []
                for slot in range(1, 10):
                    # JSON keys are strings. To retrieve them I need to index with strings.
                    slot_state = program['save_states'].get(str(slot), {'label':'(Empty Slot)'})
                    slot_states.append(slot_state)

                labels = [s['label'] for s in slot_states]
                choice_i = await InputOutputManager().select_index(labels, description = "")
                label = labels[choice_i]
                save_state_label = await InputOutputManager().user_input(
                    prompt="What do you want to call this save state?\n> ",
                    default = label if label != '(Empty Slot)' else '')
                program['save_states'][str(choice_i+1)] = deepcopy(state)
                # Save the label later because I don't want label to be part of the active state.
                program['save_states'][str(choice_i+1)]['label'] = save_state_label

                save(program, state, filepath)

                status = f"\nSaved '{save_state_label}' to slot {choice_i+1}.\n"
                print(f"🛈 saved slot {choice_i+1}", file=log_sink)

            case "Load State":

                slot_states = []
                for slot in range(1, 10):
                    slot_state = program['save_states'].get(str(slot), {'label':'(Empty Slot)'})
                    slot_states.append(slot_state)

                labels = [s.get('label', '(Unlabelled Slot)') for s in slot_states]
                choice_i = await InputOutputManager().select_index(labels, description = "")
                if labels[choice_i] == '(Empty Slot)':  
                    status = f"\nCannot load empty slot.\n"
                    continue # Go back to main menu.
                state.clear()
                state.update(slot_states[choice_i])
                state['output'] = state.get('output','') # HACK
                await InputOutputManager().write(state['output'])
                status = f"\nLoaded '{state['label']}' from slot {choice_i+1}.\n"
                print(f"🛈 Loaded slot {choice_i+1} ({labels[choice_i]}).", file=log_sink)

            case "Reload and Restart":

                _program, _state = load(filepath)
                program_args = {k:v for k,v in state['inserts'].items() if k[:3] == 'ARG' and k[3:].isnumeric()}

                # Edit the program and state dicts in place.
                program.clear()
                program.update(deepcopy(_program))
                state.clear()
                state.update(deepcopy(_state))
                state['inserts'].update(program_args)

                completion_args.clear()
                completion_args.update(deepcopy(program.get('completion_args', {})))
                named_tasks.clear()
                named_tasks.update(deepcopy(program.get('named_tasks',{})))
                await InputOutputManager().write('\n'*os.get_terminal_size().lines) # clear text
                status = f"\nRestarted Program after reloading.\n"
                print(f"🛈 Restarted Program.", file=log_sink)

            case "Quit":
                global killme
                killme = True
                t = menu_state['async_task']
                if t:
                    t.cancel()



killme = False
# we need this to kill the running task-task when opening and kill the menu-task when closing.
menu_state = {'is_menu_open': False, 'async_task': None} 
def toggle_menu():
    t = menu_state['async_task']
    if t: t.cancel()
    menu_state['is_menu_open'] = not menu_state['is_menu_open']

# TODO: this belongs in a development branch
def parse_prog_file(s):
    order = []

    def parse_phase(s) -> list:
        tasks_with_linenumbers = []
        n_chars = len(s)
        i_char = 0
        line_number = 1
        current_task = []
        phase = 'whitespace' # 'whitespace', 'single_quote', 'double_quote', 'identifier', 'bracket', or 'curly_brace'
        level = 0 # for brackets and braces.
        current_content = ''
        while i_char < n_chars:
            print(phase, ',', current_content)

            c =s[i_char]

            if c == '\n':
                line_number += 1
                tasks_with_linenumbers.append( {'task':current_task, 'line_number':line_number} )
                current_task.clear()
                assert current_content == '', f"Line {line_number}: Current content was not reset and is {current_content}."

            if phase == 'whitespace':
                assert current_content == '', f"Line {line_number}: Current content was not reset and is {current_content}."
                if c == '\'':
                    phase = 'single_quote'
                elif c == '"':
                    phase = 'double_quote'
                elif c == '[':
                    phase = 'bracket'
                elif c == '{':
                    phase = 'curly_brace'
                elif not c.isspace():
                    phase = 'identifier'
                    current_content += c

                i_char += 1
                continue

            if phase == 'single_quote':
                not_escaped = i_char == 0 or not s[i_char-1].endswith(escape)
                if c == '\'' and not_escaped:
                    phase = 'whitespace'
                    current_task.append( ('quote', current_content) )
                    current_content = ''
                else:
                    current_content += c

                i_char += 1
                continue
            
            if phase == 'double_quote':
                not_escaped = i_char == 0 or not s[i_char-1].endswith(escape)
                if c == '"' and not_escaped:
                    phase = 'whitespace'
                    current_task.append( ('quote', current_content) )
                    current_content = ''
                else:
                    current_content += c

                i_char += 1
                continue

            if phase == 'identifier':
                if c.isspace():
                    phase = 'whitespace'
                    current_task.append( current_content )
                    current_content = ''
                else:
                    current_content += c

                i_char += 1
                continue

            if phase == 'bracket':
                if c == '[':
                    level += 1
                elif c == ']':
                    level -= 1
                    if level < 0:
                        try:
                            parsed_subcontent = parse_phase(current_content)
                        except Exception as e:
                            assert False, f"Line {line_number}: Could not parse [{current_content}]."
                        phase = 'whitespace'
                        current_task.append( ('bracket', parsed_subcontent ))
                        current_content = ''
                    else:
                        current_content += c
                else:
                    current_content += c

                i_char += 1
                continue

            if phase == 'curly_brace':
                if c == '{':
                    level += 1
                elif c == '}':
                    level -= 1
                    if level < 0:
                        phase = 'whitespace'
                        try:
                            current_task.append( json5.loads('{'+current_content+'}') )
                        except ValueError as e:
                            print(f"\n\nError @ Line {line_number}: Could not parse {{{current_content}}}\n\n({str(e)})\n\nMake sure it is valid JSON5.")
                            quit()
                        current_content = ''
                    else:
                        current_content += c
                else:
                    current_content += c

                i_char += 1
                continue

        tasks_with_linenumbers.append( {'task':current_task, 'line_number':line_number} )
        current_task.clear()
        assert current_content == '', f"Line {line_number}: Current content was not reset and is {current_content}."


        return current_task

    parse_result = parse_phase(s)

    print('Parse Reult:')
    for x in parse_result:
        print(x)
    print('DONE')
    quit()


def add_line_numbers(json_content: str) -> str:
    lines = json_content.splitlines(keepends=True)
    result = []
    pattern = re.compile(r'(\bcmd\b|"cmd"|\'cmd\')\s*:\s*("(?:\\.|[^"])*"|\'(?:\\.|[^\'])*\')(\s*(?:,|\}))')
    for i, line in enumerate(lines, start=1):
        def repl(m):
            return f"{m.group(1)}:{m.group(2)}, line:{i}{m.group(3)}"
        modified_line = pattern.sub(repl, line)
        result.append(modified_line)
    return ''.join(result)


disk_program_cache = None
disk_program_hash = None


def load(filepath) -> (dict, dict):
    global disk_program_cache, disk_program_hash

    with open(filepath, 'r') as f:
        file_content = f.read()

    # Check if file changed.
    new_disk_program_hash = md5(file_content.encode()).hexdigest()
    if new_disk_program_hash == disk_program_hash:
        print(f"🛈  Load cache hit.", file=log_sink)
        program = deepcopy(disk_program_cache)
    else:
        print(f"🛈  Load cache miss.", file=log_sink)
        if filepath.endswith('.prog'):
            program = parse_prog_file(file_content)
        elif filepath.endswith('.json5'):
            file_content = add_line_numbers(file_content)
            program = json5.loads(file_content)
        else:
            assert False, f"File '{filepath}' has an unknown extension. .json5 and .prog are supported."
        validate_program(program)

        disk_program_cache = deepcopy(program)
        disk_program_hash = new_disk_program_hash
    
    # Create a deep copy so that the default state will not be affected by anything I do.
    state = deepcopy(program['default_state'])
    state['output'] = state.get('output', '')

    return program, state


def save(program, state, filepath):
    global disk_program_cache, disk_program_hash

    with open(filepath, 'r') as f:
       file_content = f.read()

    if filepath.endswith('.prog'):
        assert False, ".prog saving not implementd yet."
    new_content = splice_key_into_json5(file_content, 'save_states', program['save_states'])

    program_hash = md5(new_content.encode()).hexdigest()
    if program_hash == disk_program_hash:
        print(f"🛈  Save cache hit, no need to write.", file=log_sink)
        return
    else:
        print(f"🛈  Save cache miss.", file=log_sink)
        with open(filepath, 'w') as f:
            f.write(new_content)
    


async def async_main(filepath, args):
    assert filepath, "Specify a single program (.json5 file) to run and optionally pass arguments that the program will handle."
    filename = filepath.split('/')[-1].split('.json5')[0]
    program, state = load(filepath)

    # populate ARG1, ARG2, etc.
    for i, arg in enumerate(args, start=1):
        # always escape user passed args, if a program wants to interpolate user input
        # it can use 'unescape'.
        state['inserts'][f"ARG{i}"] = (arg
            .replace(insert_start, escape+insert_start)
            .replace(insert_stop, escape+insert_stop))

    completion_args = program.get('completion_args', {})
    named_tasks = program.get('named_tasks', {})

    if len(program['order']) > 0:
        await InputOutputManager().start()
        await asyncio.sleep(0)
        await InputOutputManager().write(state.get('output',''))

    while state['order_index'] <= len(program['order']): # order_index is 1-based.

        loop = asyncio.get_running_loop()
        loop.add_signal_handler(SIGINT, toggle_menu)

        if menu_state['is_menu_open']:
            menu_state['async_task'] = loop.create_task( main_menu(program, state, completion_args, named_tasks, filepath) )
        else:
            task = program['order'][state['order_index']-1] # -1 because order_index is 1-based.

            # this is necessary if user opened menu during printing
            await InputOutputManager().clear()
            await InputOutputManager().write(state['output'])
            menu_state['async_task'] = loop.create_task( execute_task(state, task, completion_args, named_tasks, f"{task['traceback_label']}") )

        try:
            result = await menu_state["async_task"]
            match result:
                case None:
                    state['order_index'] += 1
                case {'goto_target': goto_target}:
                    state['order_index'] = 2 + min( # +1 for 1-indexing and +1 to go past the label
                        i for i in range(len(program['order']))
                        if program['order'][i]['cmd'] == 'label' and program['order'][i]['name'] == goto_target)

                case somethingelse:
                    raise Exception(f"Task {task} returned unexpected value: {somethingelse}.")

        except asyncio.CancelledError:
            pass

        if killme:
            print(f"🛈 Terminated by user.", file=log_sink)
            break

    else:
        print(f"🛈 Reached end of order list.", file=log_sink)

    # IOM only gets started if at least one task exists.
    if len(program['order']) > 0:
        await InputOutputManager().stop()

    print(state['output'].strip())

    return state

def main(): # cli entry point


    parser = argparse.ArgumentParser(
        description="Run an interpolation-engine program.",
        allow_abbrev=False,
    )
    parser.add_argument("program", nargs="?", help="Path to the .json5 program file.")
    parser.add_argument(
        "program_arguments",
        nargs="*",
        help=(
            "Extra positional arguments passed to the program and accessible via '{ARG1}', '{ARG2}', etc. "
            "Use '--' before arguments that start with '-'."
        ),
    )
    parser.add_argument("--log", dest="log_path", help="Specify a path to store log info at (recommended).")
    parser.add_argument("--history", dest="prompt_history", help="Path to store input history at. Settings this allows you to re-enter inputs from other sessions by hitting UP.")
    parser.add_argument(
        "--inserts-dir",
        dest="inserts_dir",
        help="Optional directory to load inserts from when a key is not found in state['inserts'].",
    )
    parser.add_argument(
        "--agent-mode",
        dest="agent_mode",
        action="store_true",
        help="Wait for user_input/user_choice via /tmp/agent_input and write context to /tmp/agent_output.",
    )
    args = parser.parse_args()

    global log_sink, prompt_history_path
    log_sink = open(args.log_path, "a") if args.log_path else open(os.devnull, 'w')
    prompt_history_path = args.prompt_history if args.prompt_history else None
    InputOutputManager.agent_mode = args.agent_mode

    if not args.program:
        print("Error: specify a program (.json5 file) to run.")
        return
    if args.inserts_dir:
        if not os.path.isdir(args.inserts_dir):
            print(f"Error: --inserts-dir must be an existing directory, got '{args.inserts_dir}'.")
            return
        global inserts_dir
        inserts_dir = args.inserts_dir

    state = asyncio.run(async_main(args.program, args.program_arguments))

if __name__ == '__main__':
    main()
