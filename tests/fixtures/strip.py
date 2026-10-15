#!/usr/bin/env python3
"""Strip a GLB down to nodes, skins and animations so it can live in the test suite.

Usage: python3 -I tests/fixtures/strip.py SOURCE.glb DESTINATION.glb
"""

import json
import struct
import sys

COMPONENT_SIZES = {5120: 1, 5121: 1, 5122: 2, 5123: 2, 5125: 4, 5126: 4}
WIDTHS = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT2": 4, "MAT3": 9, "MAT4": 16}
DROPPED = ("meshes", "materials", "textures", "images", "samplers", "cameras", "extensionsUsed", "extensionsRequired")


def read(path):
    data = open(path, "rb").read()
    json_length = struct.unpack_from("<I", data, 12)[0]
    document = json.loads(data[20 : 20 + json_length])
    binary_start = 20 + json_length + 8
    binary_length = struct.unpack_from("<I", data, 20 + json_length)[0]
    return document, data[binary_start : binary_start + binary_length]


def write(path, document, binary):
    json_bytes = json.dumps(document, separators=(",", ":")).encode()
    json_bytes += b" " * (-len(json_bytes) % 4)
    binary += b"\0" * (-len(binary) % 4)
    total = 12 + 8 + len(json_bytes) + 8 + len(binary)
    with open(path, "wb") as file:
        file.write(struct.pack("<III", 0x46546C67, 2, total))
        file.write(struct.pack("<II", len(json_bytes), 0x4E4F534A) + json_bytes)
        file.write(struct.pack("<II", len(binary), 0x004E4942) + binary)


def main(source, destination):
    document, binary = read(source)
    accessors, views, packed, copied = [], [], bytearray(), {}

    def copy(index):
        if index in copied:
            return copied[index]
        accessor = dict(document["accessors"][index])
        view = document["bufferViews"][accessor.pop("bufferView")]
        element_size = COMPONENT_SIZES[accessor["componentType"]] * WIDTHS[accessor["type"]]
        stride = view.get("byteStride") or element_size
        start = view.get("byteOffset", 0) + accessor.pop("byteOffset", 0)
        packed.extend(b"\0" * (-len(packed) % 4))
        offset = len(packed)
        for element in range(accessor["count"]):
            packed.extend(binary[start + element * stride : start + element * stride + element_size])
        views.append({"buffer": 0, "byteOffset": offset, "byteLength": len(packed) - offset})
        accessor["bufferView"] = len(views) - 1
        accessors.append(accessor)
        copied[index] = len(accessors) - 1
        return copied[index]

    for skin in document.get("skins", []):
        if "inverseBindMatrices" in skin:
            skin["inverseBindMatrices"] = copy(skin["inverseBindMatrices"])
    for animation in document.get("animations", []):
        for sampler in animation["samplers"]:
            sampler["input"] = copy(sampler["input"])
            sampler["output"] = copy(sampler["output"])
    for node in document.get("nodes", []):
        for key in ("mesh", "camera", "extensions"):
            node.pop(key, None)
    for key in DROPPED:
        document.pop(key, None)
    document["accessors"] = accessors
    document["bufferViews"] = views
    document["buffers"] = [{"byteLength": len(packed)}]
    write(destination, document, bytes(packed))


if __name__ == "__main__":
    main(*sys.argv[1:3])
