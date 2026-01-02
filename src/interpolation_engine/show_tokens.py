from copy import deepcopy
from .filter import inverted_filter, filter
from glob import glob
from hashlib import md5
from pydantic import BaseModel
from signal import SIGINT
from typing import Literal
import argparse
import json
from openai import OpenAI
import os
import random # for random.choice
import re
import sys
from datetime import datetime # for the 'HH:MM' special insertkey
from typing import Literal
from pydantic import BaseModel

from prompt_toolkit import print_formatted_text
from prompt_toolkit.formatted_text import FormattedText

def main(): # cli entry point


    parser = argparse.ArgumentParser(
        description="Show the tokens of an input text.",
        allow_abbrev=False,
    )
    parser.add_argument("model", nargs=1, help="The model to use")
    parser.add_argument("text", nargs=1, help="The text to tokenize.")
    parser.add_argument("--api-url", default="http://localhost:8080", dest="api_url", nargs="?", help="API URL, localhost:8080 for llama.cpp local.")
    parser.add_argument("--api-key", default="(unused)", dest="api_key", nargs="?", help="API key, not needed for unrestricted for local servers.")
    args = parser.parse_args()


    if not args.text:
        print("Error: Specify a text to tokenize.")
        return
    assert len(args.text) == len(args.model) == 1
    args.text = args.text[0]
    args.model = args.model[0]

    client = OpenAI(base_url=args.api_url, api_key=args.api_key)
    class ValidResponse(BaseModel):
        # Literal wants a tuple not a list.
        text: Literal[args.text]

    schema = json.dumps(ValidResponse.model_json_schema())
    prompt = f"Respond only with the following JSON object and nothing else:\n\n{json.dumps({'text':args.text})}"

    response =  client.chat.completions.create(
        messages=[{'role': 'user', 'content': prompt}],
        model=args.model,
        stream=True,
        response_format={'type':'json_schema', 'json_schema':schema},
    )


    colors = ('green', 'yellow','cyan', 'red',)
    raw = ''
    for i,comp in enumerate(response):

        chunk = comp.choices[0]
        delta = chunk.delta.content
        if not delta is None:
            color = colors[i % len(colors)]
            print_formatted_text(FormattedText([(color, delta)]), end='')
            raw += delta

    returned = ValidResponse.model_validate_json(raw).text

    if not args.text == returned:
        print("\nWarning: Input text was not reproduced exactly!")

if __name__ == '__main__':
    main()


