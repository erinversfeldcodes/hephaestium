#!/usr/bin/env python3
"""Grade a strict Protobuf JSON reader against the runtime contracts' cases.

    check-contract-cases.py DESCRIPTOR CASES

DESCRIPTOR is a file descriptor set from `buf build -o`. CASES holds one
directory for each message, named for it, with a `valid` and an `invalid`
directory of JSON in the Protobuf JSON mapping, which is the wire form.

Every valid case must be accepted. Every invalid case must be rejected, and for
the reason it states in a sidecar of the same name with the extension
`.violates`, one line:

    <kind> <what>

where <kind> is the rule it breaks and <what> is a word the report must
mention, such as the field that is not allowed. A case that is rejected for
some other reason proves nothing about the rule it was written for.

Why a reader of our own and not `buf convert`: it discards unknown fields and
unknown enum names, so a misspelt field passes. A contract that accepts a
misspelt field fails silently in the field, which is the failure these cases
exist to prevent. This reader is strict, and a reader in any other language is
graded by the same cases and brings its own table of how it words each kind.
"""
import re
import sys
from pathlib import Path

from google.protobuf import descriptor_pb2, descriptor_pool, json_format, message_factory

PACKAGE = "hephaestium.v1alpha1"

# How this reader words each kind of violation. A pattern is matched against
# the error the library raises.
WORDING = {
    "unknown-field": r'no field named "',
    "bad-enum": r"Invalid enum value|Failed to parse .* enum|invalid enum",
    "bad-duration": r"duration|Duration",
    "bad-timestamp": r"time data|Timestamp|timestamp",
    "bad-number": r"invalid literal for int|Couldn.t parse (integer|float)|could not convert|out of range",
    "wrong-type": r"Failed to parse|expected|Expected|has type|not a|Struct",
    "multiple-oneof": r"oneof",
}


def load_pool(path: Path) -> descriptor_pool.DescriptorPool:
    descriptors = descriptor_pb2.FileDescriptorSet()
    descriptors.ParseFromString(path.read_bytes())
    pool = descriptor_pool.DescriptorPool()
    for file in descriptors.file:
        pool.Add(file)
    return pool


def parse(pool: descriptor_pool.DescriptorPool, message: str, text: str) -> None:
    descriptor = pool.FindMessageTypeByName(f"{PACKAGE}.{message}")
    instance = message_factory.GetMessageClass(descriptor)()
    json_format.Parse(text, instance, ignore_unknown_fields=False)


def statement_of(case: Path) -> tuple[str, str]:
    sidecar = case.with_suffix(".violates")
    if not sidecar.is_file():
        raise SystemExit(f"{case}: has no {sidecar.name} stating the rule it breaks")
    words = sidecar.read_text().split(None, 1)
    if len(words) != 2 or words[0] not in WORDING:
        raise SystemExit(
            f"{sidecar}: must be '<kind> <what>' with a kind from {sorted(WORDING)}"
        )
    return words[0], words[1].strip()


def main() -> int:
    descriptor, cases = Path(sys.argv[1]), Path(sys.argv[2])
    pool = load_pool(descriptor)
    status = 0
    accepted = rejected = 0

    for directory in sorted(p for p in cases.iterdir() if p.is_dir()):
        message = directory.name
        for case in sorted((directory / "valid").glob("*.json")):
            try:
                parse(pool, message, case.read_text())
                accepted += 1
            except Exception as error:  # the library raises several kinds
                print(f"{case}: a valid case was rejected: {error}", file=sys.stderr)
                status = 1

        invalid = directory / "invalid"
        for orphan in sorted(invalid.glob("*.violates")):
            if not orphan.with_suffix(".json").is_file():
                print(f"{orphan}: states a rule for a case that does not exist", file=sys.stderr)
                status = 1
        for case in sorted(invalid.glob("*.json")):
            kind, what = statement_of(case)
            try:
                parse(pool, message, case.read_text())
            except Exception as error:
                report = str(error)
                if not re.search(WORDING[kind], report):
                    print(
                        f"{case}: rejected for the wrong reason\n  stated: {kind} {what}\n  reported: {report}",
                        file=sys.stderr,
                    )
                    status = 1
                elif what.lower() not in report.lower():
                    print(
                        f"{case}: rejected as {kind}, but the report does not mention '{what}'\n  reported: {report}",
                        file=sys.stderr,
                    )
                    status = 1
                else:
                    rejected += 1
            else:
                print(f"{case}: was accepted, but must be rejected as {kind}", file=sys.stderr)
                status = 1

    if status == 0:
        print(f"{accepted} accepted, {rejected} rejected for the reason each states")
    return status


if __name__ == "__main__":
    sys.exit(main())
