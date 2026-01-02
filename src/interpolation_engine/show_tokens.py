import argparse
from openai import OpenAI
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

    prompt = f"Respond only with the following text and nothing else:\n\n<start>{args.text}<end>\n\nDo not include <start> and <end>."

    response =  client.chat.completions.create(
        messages=[{'role': 'user', 'content': prompt}],
        model=args.model,
        stream=True,
    )


    colors = ('yellow','cyan',)
    raw = ''
    for i,comp in enumerate(response):

        chunk = comp.choices[0]
        delta = chunk.delta.content
        if not delta is None:
            color = colors[i % len(colors)]
            shown_delta = delta.replace('\n', '\\n')
            print_formatted_text(FormattedText([(f"bg:{color} fg:black", shown_delta)]), end='')
            raw += delta


    print()
    returned = raw.strip()
    if not args.text == returned:
        print("\nWarning: Input text was not reproduced exactly!")

if __name__ == '__main__':
    main()
