#!/usr/bin/env python3
"""X11 capture worker. stdout is a JSON-lines protocol, never logging."""
import base64
import io
import json
import os
import sys

from PIL import Image, ImageGrab

for line in sys.stdin:
    request = {}
    try:
        request = json.loads(line)
        image = ImageGrab.grab(xdisplay=os.environ.get("DISPLAY", ":99")).convert("RGB")
        width, height = image.size
        if not (1 <= width <= 8192 and 1 <= height <= 8192):
            raise ValueError("unsupported display geometry")
        image.thumbnail((1920, 1200), Image.Resampling.LANCZOS)
        encoded = io.BytesIO()
        image.save(encoded, format="JPEG", quality=90)
        result = {
            "id": request["id"],
            "displayWidth": width, "displayHeight": height,
            "imageWidth": image.width, "imageHeight": image.height,
            "jpeg": base64.b64encode(encoded.getvalue()).decode("ascii"),
        }
        print(json.dumps(result), flush=True)
    except Exception as error:
        print(json.dumps({"id": request.get("id"), "error": str(error)}), flush=True)
