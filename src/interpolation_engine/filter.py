
def filter(start_str, stop_str, enumerate_outputs = False):
    # store buffers in lists to pass by reference
    mode_is_shown = [False] # toggle for state machine>
    # if true, print everything until start_str.
    # if false, print nothing until stop_str.

    buffer = [""]
    outputs = []

    def update(chunk):

        # 1. ACCEPT EVERYTHING UP TO start_str
        buffer[0] = buffer[0] + chunk
        safebelow = None
        next_str = stop_str if mode_is_shown[0] else start_str

        enumeration = ''
        if buffer[0].startswith(next_str) and next_str != '':
            # toggle
            buffer[0] = buffer[0][len(next_str):]
            mode_is_shown[0] = not mode_is_shown[0]
            #print(f'toggling to {mode_is_shown[0]} and appending')

            if mode_is_shown[0]:
                outputs.append('')
                if enumerate_outputs:
                    enumeration = "\n\n"*(len(outputs) > 1) + f"{len(outputs)}. "

        for safebelow in range(len(buffer[0])):
            if next_str.startswith(buffer[0][safebelow:safebelow+len(next_str)]) and next_str != '':
                break
        else:
            safebelow = len(buffer[0])

        delta = buffer[0][:safebelow] if mode_is_shown[0] else ''
        buffer[0] = buffer[0][safebelow:]

        if mode_is_shown[0]:
            outputs[-1] += delta

        return enumeration + delta

    def passthrough(chunk):
        if len(outputs) == 0:
            outputs.append('')
        outputs[-1] += chunk

        return chunk

    if start_str == '' or stop_str == '':
        return passthrough, outputs
    else:
        return update, outputs



def inverted_filter(start_str, stop_str, perdelta = lambda s: print(s, end='')):
    # store buffers in lists to pass by reference
    mode_is_shown = [True] # toggle for state machine>
    # if true, print everything until start_str.
    # if false, print nothing until stop_str.

    buffer = [""]

    def update(chunk):

        # 1. ACCEPT EVERYTHING UP TO start_str
        buffer[0] = buffer[0] + chunk
        safebelow = None
        next_str = start_str if mode_is_shown[0] else stop_str

        if buffer[0].startswith(next_str) and next_str != '':
            # toggle
            buffer[0] = buffer[0][len(next_str):]
            mode_is_shown[0] = not mode_is_shown[0]

        for safebelow in range(len(buffer[0])):
            if next_str.startswith(buffer[0][safebelow:safebelow+len(next_str)]) and next_str != '':
                break
        else:
            safebelow = len(buffer[0])
        delta = buffer[0][:safebelow] if mode_is_shown[0] else ''
        buffer[0] = buffer[0][safebelow:]


        return delta

    return update


if __name__ == "__main__":
    # TODO: remove or put this in tests
    from time import sleep
    #sample = "0<secret>1</secret>2<secret>3</secret>4"
    sample = "<output>1</output>\n\n\t<output>and 2</output>"
    #sample = "<secret>Hey how</secret>falskfjasldfkj<secret> are you?</secret>"
    #inv_f = inverted_filter("<secret>","</secret>")
    #inv_f = inverted_filter("","")
    #f,outputs = filter("<secret>","</secret>", enumerate_outputs=True)
    f,outputs = filter("<output>","</output>", enumerate_outputs=True)
    chunk_width = 3
    for i in range(0, len(sample), chunk_width):
        chunk = sample[i:i+chunk_width]
        print(f(chunk), end='', flush=True)
        sleep(0.03)

    print(f"\n\nGot {len(outputs)} outputs:")
    for o in outputs:
        print('\n    - '+o)
