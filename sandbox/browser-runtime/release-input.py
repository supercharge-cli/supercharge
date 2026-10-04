#!/usr/bin/env python3
"""Release server-side X11 input after a control owner disappears."""
import os
from Xlib import X, display
from Xlib.ext import xtest

connection = display.Display(os.environ.get("DISPLAY", ":99"))
keys = connection.query_keymap()
for keycode in range(8, 256):
    if keys[keycode // 8] & (1 << (keycode % 8)):
        xtest.fake_input(connection, X.KeyRelease, keycode)
for button in (1, 2, 3):
    xtest.fake_input(connection, X.ButtonRelease, button)
connection.sync()
connection.close()
