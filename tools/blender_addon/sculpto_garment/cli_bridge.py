"""The one driver for every `sculpto` command the add-on issues.

Imports no `bpy`, so the argv/envelope contract is testable with plain
`python3` (tools/tests/test_sculpto_cli_bridge.py). All format reading and
writing lives behind the command surface (DECL0002): the add-on sends one JSON
payload per command and receives the engine's one-line envelope from stdout.
Exit codes are part of the contract: 0 success, 2 invalid request, 1 refusal
or environment failure (crates/sculpto-cli/src/run.rs).
"""

import json
import os
import subprocess
from pathlib import Path

# Seconds a command may run before the bridge gives up on it.
DEFAULT_TIMEOUT = 300.0


class EnvelopeError(Exception):
    """Stdout was not the one JSON envelope the command contract promises."""


class CommandError(Exception):
    """The envelope reported failure: the engine's kind, code and message."""

    def __init__(self, kind, code, message, exit_code=None):
        super().__init__(message)
        self.kind = kind
        self.code = code
        self.message = message
        self.exit_code = exit_code

    def __str__(self):
        location = f" (exit {self.exit_code})" if self.exit_code is not None else ""
        return f"[{self.kind}/{self.code}]{location} {self.message}"


def find_binary(root=None):
    """Locate the `sculpto` binary: SCULPTO_BIN, the debug build, then PATH."""
    env = os.environ.get("SCULPTO_BIN")
    if env:
        return env
    if root is None:
        # tools/blender_addon/sculpto_garment/cli_bridge.py -> repo root
        root = Path(__file__).resolve().parents[3]
    candidate = Path(root) / "target" / "debug" / "sculpto"
    if candidate.is_file():
        return str(candidate)
    return "sculpto"


def export_obj_payload(nif, out, shape=None):
    """`export.obj` input: read one NIF shape and write it as an OBJ."""
    payload = {"nif": str(nif), "out": str(out)}
    if shape:
        payload["shape"] = shape
    return payload


def export_nif_payload(nif, out, target, shape=None):
    """`export.nif` input: re-encode one shape for the `se` or `le` target."""
    payload = {"nif": str(nif), "out": str(out), "target": target}
    if shape:
        payload["shape"] = shape
    return payload


def bake_payload(project, presets, set_name, preset, endpoint, out, shape=None, home=None):
    """`bake` input: apply a resolved preset to a source NIF, write an OBJ."""
    payload = {
        "project": str(project),
        "presets": str(presets),
        "set": set_name,
        "preset": preset,
        "endpoint": endpoint,
        "out": str(out),
    }
    if shape:
        payload["shape"] = shape
    if home:
        payload["home"] = str(home)
    return payload


def fit_payload(nif, shape, body_shape, body, out):
    """`outfit.fit` input: bind a garment shape onto a baked body OBJ."""
    return {
        "nif": str(nif),
        "shape": shape,
        "body_shape": body_shape,
        "body": str(body),
        "out": str(out),
    }


def build_run_argv(binary, command, payload):
    """The exact argv: `sculpto run --cmd <id> --input <json>`.

    A `None` payload omits `--input`; the engine then dispatches null.
    """
    argv = [str(binary), "run", "--cmd", command]
    if payload is not None:
        argv += ["--input", json.dumps(payload)]
    return argv


def parse_envelope(stdout):
    """Return the `result` member of the one envelope on stdout.

    Raises EnvelopeError when stdout is not exactly one JSON envelope and
    CommandError when the envelope reports `ok: false`.
    """
    try:
        envelope = json.loads(stdout.strip())
    except (json.JSONDecodeError, ValueError) as error:
        raise EnvelopeError(f"stdout is not one JSON envelope: {error}: {stdout[:200]!r}") from error
    if not isinstance(envelope, dict) or "ok" not in envelope:
        raise EnvelopeError(f"stdout has no envelope shape: {stdout[:200]!r}")
    if envelope["ok"] is True:
        return envelope.get("result")
    error = envelope.get("error") or {}
    raise CommandError(
        error.get("kind", "unknown"),
        error.get("code", "unknown"),
        error.get("message", "the envelope carried no message"),
    )


class SculptoCli:
    """Runs commands and returns envelope results; the only subprocess here."""

    def __init__(self, binary=None, timeout=DEFAULT_TIMEOUT):
        self.binary = binary or find_binary()
        self.timeout = timeout

    def run(self, command, payload=None):
        """Dispatch one command id and return its result member.

        Raises EnvelopeError on a contract breach and CommandError with the
        engine's kind/code/message on a refused or failed command; the
        process exit code rides along (0 success, 2 invalid request, 1
        refusal or environment failure).
        """
        argv = build_run_argv(self.binary, command, payload)
        try:
            completed = subprocess.run(
                argv, capture_output=True, text=True, timeout=self.timeout, check=False
            )
        except (OSError, subprocess.TimeoutExpired):
            raise
        try:
            result = parse_envelope(completed.stdout)
        except CommandError as error:
            error.exit_code = completed.returncode
            raise
        if completed.returncode != 0:
            raise EnvelopeError(
                f"{command}: envelope reported ok but the exit code is {completed.returncode}"
            )
        return result


def run_cli(operator, command, payload, cli=None):
    """Run one command for an operator; every failure is a reported error.

    A refused command surfaces the engine's own classification as
    `kind: message` (F06) through `operator.report` and returns None, so the
    caller returns {'CANCELLED'} — the engine's kind, not a traceback. Bridge
    breaches (envelope shape, ok envelope with a failure exit), a missing
    binary and a timeout report the same way. Returns the result member on
    success.
    """
    try:
        return (cli or SculptoCli()).run(command, payload)
    except CommandError as error:
        operator.report({"ERROR"}, f"{error.kind}: {error.message}")
    except EnvelopeError as error:
        operator.report({"ERROR"}, f"protocol: {error}")
    except (OSError, subprocess.TimeoutExpired) as error:
        operator.report({"ERROR"}, f"launch: {error}")
    return None
