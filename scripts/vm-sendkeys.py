#!/usr/bin/env python3
"""Type text / send keys into a libvirt guest via `virsh send-key` (Linux keycodes).

Autonomous VM driving for the y22i Windows-crash validation: the er-effects-win11 guest has no
qemu-guest-agent and no host->guest port forward, so the only host-driven input channel is
virsh send-key. Verify effects out-of-band (host virsh screenshot pixel telemetry, or the host
HTTP-server access log when a typed command fetches from 10.0.2.2).

Usage:
  vm-sendkeys.py text "some string"     # type a literal string (maps chars -> keycodes)
  vm-sendkeys.py key KEY_ENTER          # one keycode
  vm-sendkeys.py chord KEY_LEFTMETA KEY_R   # keys pressed together (Win+R)
Env: DOMAIN (default er-effects-win11), LIBVIRT_DEFAULT_URI (default qemu:///system), HOLDTIME ms.
"""
import os, subprocess, sys, time

DOMAIN = os.environ.get('DOMAIN', 'er-effects-win11')
os.environ.setdefault('LIBVIRT_DEFAULT_URI', 'qemu:///system')
HOLD = os.environ.get('HOLDTIME', '40')

_LOWER = {c: f'KEY_{c.upper()}' for c in 'abcdefghijklmnopqrstuvwxyz'}
_DIGIT = {c: f'KEY_{c}' for c in '0123456789'}
# unshifted punctuation -> keycode
_PUNCT = {
    ' ': 'KEY_SPACE', '.': 'KEY_DOT', ',': 'KEY_COMMA', '/': 'KEY_SLASH', '-': 'KEY_MINUS',
    ';': 'KEY_SEMICOLON', "'": 'KEY_APOSTROPHE', '\\': 'KEY_BACKSLASH', '=': 'KEY_EQUAL',
    '[': 'KEY_LEFTBRACE', ']': 'KEY_RIGHTBRACE', '`': 'KEY_GRAVE',
}
# shifted characters -> base keycode (sent with KEY_LEFTSHIFT)
_SHIFT = {
    '!': 'KEY_1', '@': 'KEY_2', '#': 'KEY_3', '$': 'KEY_4', '%': 'KEY_5', '^': 'KEY_6',
    '&': 'KEY_7', '*': 'KEY_8', '(': 'KEY_9', ')': 'KEY_0', '_': 'KEY_MINUS', '+': 'KEY_EQUAL',
    ':': 'KEY_SEMICOLON', '"': 'KEY_APOSTROPHE', '|': 'KEY_BACKSLASH', '?': 'KEY_SLASH',
    '<': 'KEY_COMMA', '>': 'KEY_DOT', '{': 'KEY_LEFTBRACE', '}': 'KEY_RIGHTBRACE', '~': 'KEY_GRAVE',
}


def keycodes_for_char(ch):
    if ch in _LOWER:
        return [_LOWER[ch]]
    if ch in _DIGIT:
        return [_DIGIT[ch]]
    if ch.isupper():
        return ['KEY_LEFTSHIFT', f'KEY_{ch}']
    if ch in _PUNCT:
        return [_PUNCT[ch]]
    if ch in _SHIFT:
        return ['KEY_LEFTSHIFT', _SHIFT[ch]]
    raise ValueError(f'unmappable char: {ch!r}')


def send(codes):
    subprocess.run(['virsh', 'send-key', DOMAIN, '--holdtime', HOLD, *codes], check=True,
                   capture_output=True, timeout=15)
    time.sleep(0.05)  # keystroke pacing for the guest input queue, not synchronization


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    mode = sys.argv[1]
    if mode == 'text':
        s = sys.argv[2]
        for ch in s:
            send(keycodes_for_char(ch))
        print(f'typed {len(s)} chars')
    elif mode == 'key':
        send([sys.argv[2]])
        print(f'sent {sys.argv[2]}')
    elif mode == 'chord':
        send(sys.argv[2:])
        print(f'chord {" ".join(sys.argv[2:])}')
    else:
        print('unknown mode', mode)
        sys.exit(2)


if __name__ == '__main__':
    main()
