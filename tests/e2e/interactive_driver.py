"""Drive two model turns on an isolated controlling PTY without a transcript."""

from __future__ import annotations

from dataclasses import dataclass
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import sys
import termios
from time import monotonic
from uuid import uuid4


# Harness waits, widened for a loaded host; they never extend the worker deadline.
CONTROL_WAIT = 60
SUBMIT_PAUSE = 0.7
QUIET_WINDOW = 0.7
QUIET_LIMIT = 18
STATUS = struct.Struct("!i")
PTY_READ_SIZE = 4096
# A recognised startup screen is answered after it settled, one key at a time.
# Both pauses exceed the 150 ms type-ahead guard of Claude Code 2.1.329 dialogs.
SETTLE = 1.5
KEY_GAP = 1.0
MAX_ANSWERS = 3
ANSWER_COOLDOWN = 4.0
TIMELINE_LIMIT = 96
ACTIVITY_WINDOW = 30.0
KEYS = {"enter": b"\r", "up": b"\x1b[A", "down": b"\x1b[B", "escape": b"\x1b"}


@dataclass(frozen=True)
class Screen:
    name: str
    needle: bytes
    keys: tuple = ()


def screen(name, text, *keys):
    """Match visible glyphs only: TUIs may draw spaces as cursor moves."""
    return Screen(name, bytes(byte for byte in text.encode("utf-8") if byte > 0x20), keys)


# Claude Code 2.1.329 with a new CLAUDE_CONFIG_DIR and an API key in its environment shows the
# onboarding steps (theme, api-key, security notes) and then the workspace trust dialog. The
# answers keep the selected profile key, trust the sandbox copy and decline optional setup.
CLAUDE_SCREENS = (
    screen("claude.theme", "Let's get started.", "enter"),
    screen("claude.api_key", "Detected a custom API key in your environment", "up", "enter"),
    screen("claude.security_notes", "Security notes:", "enter"),
    screen("claude.terminal_setup", "Use Claude Code's terminal setup?", "escape"),
    screen("claude.workspace_trust", "Quick safety check", "down", "enter"),
    screen("claude.powerups", "Take the tour", "escape"),
    screen("claude.welcome", "Welcome to Claude Code"),
    screen("claude.connectivity_failed", "Unable to connect to Anthropic services"),
    screen("claude.composer", "? for shortcuts"),
    screen("claude.api_error", "API Error"),
    screen("claude.invalid_api_key", "Invalid API key"),
    screen("claude.login_required", "Please run /login"),
    screen("claude.not_logged_in", "Not logged in"),
    screen("claude.login_method", "Select login method"),
    screen("claude.model_issue", "There's an issue with the selected model"),
    screen("claude.credit", "Credit balance is too low"),
    screen("claude.unreachable", "Can't reach the API server"),
    screen("claude.request_timeout", "Request timed out"),
)
# Codex 0.170.0 screens are recorded only: answering a model migration or an update
# choice could change the requested model or the installation.
CODEX_SCREENS = (
    screen("codex.composer", "Ask Codex to do anything"),
    screen("codex.model_migration", "Codex just got an upgrade"),
    screen("codex.update", "Update available!"),
    screen("codex.welcome", "Welcome to Codex"),
    screen("codex.sign_in", "Sign in with ChatGPT"),
    screen("codex.continue_anyway", "Continue anyway?"),
    screen("codex.unexpected_status", "unexpected status"),
    screen("codex.stream_disconnected", "stream disconnected"),
    screen("codex.reconnecting", "Reconnecting"),
    screen("codex.stream_error", "stream error"),
    screen("codex.retrying", "Retrying"),
)
COMMON_SCREENS = (screen("working", "to interrupt"),)
SCREENS = {"claude-code": CLAUDE_SCREENS + COMMON_SCREENS, "codex": CODEX_SCREENS + COMMON_SCREENS}


@dataclass(frozen=True)
class InteractiveProof:
    stage: str
    receipts: int
    cli_exit_status: int | None
    termios_restored: bool
    reason: str | None = None
    # Fixed screen names, key names and stage events with offsets; never terminal text.
    timeline: tuple = ()

    @property
    def passed(self):
        return (
            self.stage == "complete"
            and self.receipts == 2
            and self.cli_exit_status == 0
            and self.termios_restored
        )


class PtyError(Exception):
    pass


class Compactor:
    """Keep visible glyphs only, without escape sequences, controls or spaces."""

    def __init__(self):
        self.escape = ""

    def feed(self, chunk):
        visible = bytearray()
        for byte in chunk:
            if self.escape == "csi":
                if 0x40 <= byte <= 0x7E:
                    self.escape = ""
            elif self.escape in {"string", "string_escape"}:
                if byte == 0x07 or (self.escape == "string_escape" and byte == 0x5C):
                    self.escape = ""
                else:
                    self.escape = "string_escape" if byte == 0x1B else "string"
            elif self.escape == "charset":
                self.escape = ""
            elif self.escape == "escape":
                if byte == 0x5B:
                    self.escape = "csi"
                elif byte in b"]PX^_":
                    self.escape = "string"
                elif byte in b"()*+-./#%":
                    self.escape = "charset"
                else:
                    self.escape = ""
            elif byte == 0x1B:
                self.escape = "escape"
            elif byte > 0x20 and byte != 0x7F:
                visible.append(byte)
        return bytes(visible)


class Responder:
    """Recognise fixed startup screens and schedule their fixed answers, without a transcript."""

    def __init__(self, screens, started):
        self.screens = tuple(screens)
        self.started = started
        self.compactor = Compactor()
        self.tail = b""
        self.width = max((len(item.needle) for item in self.screens), default=1)
        self.seen = set()
        self.answers = {}
        self.answered_at = {}
        self.pending = None
        self.output_seen = False
        self.timeline = []
        # Byte counts per window only: they show whether the screen kept changing, never what it showed.
        self.window_started = started
        self.window_bytes = 0
        self.total_bytes = 0

    def note(self, event, now=None):
        if len(self.timeline) < TIMELINE_LIMIT:
            offset = (monotonic() if now is None else now) - self.started
            self.timeline.append(f"+{offset:.1f}s {event}")

    def activity(self, now, final=False):
        if self.window_bytes and (final or now - self.window_started >= ACTIVITY_WINDOW):
            self.note(f"output:{self.window_bytes}B", now)
            self.window_bytes = 0
        if final or now - self.window_started >= ACTIVITY_WINDOW:
            self.window_started = now

    def observe(self, chunk, now, *, allow_answers):
        if chunk and not self.output_seen:
            self.output_seen = True
            self.note("output:first", now)
        self.window_bytes += len(chunk)
        self.total_bytes += len(chunk)
        self.activity(now)
        visible = self.compactor.feed(chunk)
        if not visible:
            return
        window = self.tail + visible
        latest = None
        for item in self.screens:
            index = window.rfind(item.needle, max(0, len(self.tail) - len(item.needle) + 1))
            if index < 0:
                continue
            if (not item.keys or not allow_answers) and item.name not in self.seen:
                self.seen.add(item.name)
                self.note(f"screen:{item.name}", now)
            if (allow_answers and item.keys and self.answers.get(item.name, 0) < MAX_ANSWERS
                    and now - self.answered_at.get(item.name, now - ANSWER_COOLDOWN) >= ANSWER_COOLDOWN
                    and (latest is None or index > latest[0])):
                latest = (index, item)
        # A redrawn screen keeps its first sighting, so animation never postpones its answer.
        if latest is not None and (self.pending is None or self.pending[0] is not latest[1]):
            self.note(f"screen:{latest[1].name}", now)
            self.seen.add(latest[1].name)
            self.pending = (latest[1], now)
        self.tail = window[-(self.width - 1):] if self.width > 1 else b""

    def due(self, now):
        return self.pending is not None and now - self.pending[1] >= SETTLE

    def take(self, now):
        item = self.pending[0]
        self.pending = None
        self.answers[item.name] = self.answers.get(item.name, 0) + 1
        self.answered_at[item.name] = now
        return item


class Marker:
    """Recognize streamed text and track concealment across reads and turns."""

    def __init__(self, text=None):
        self.expect(text)
        self.escape = ""
        self.parameters = bytearray()
        self.concealed = False

    def expect(self, text):
        self.expected = text.encode("ascii") if text else b""
        self.matched = 0

    def _style(self):
        parts = self.parameters.split(b";")
        if len(self.parameters) > 256 or any(part and not part.isdigit() for part in parts):
            self.matched = 0
            return
        parameters = [int(part or b"0") for part in parts]
        index = 0
        while index < len(parameters):
            value = parameters[index]
            if value in (38, 48):
                # Extended color arguments can contain 8, 28 or 0.
                mode = parameters[index + 1:index + 2]
                if mode == [5]:
                    index += 3
                elif mode == [2]:
                    index += 5
                else:
                    self.matched = 0
                    return
                continue
            if value == 8:
                self.concealed = True
            elif value in (0, 28):
                self.concealed = False
            index += 1

    def feed(self, chunk):
        observed = False
        for byte in chunk:
            if self.escape == "csi":
                if 0x40 <= byte <= 0x7E:
                    self.escape = ""
                    if byte == ord("m"):
                        self._style()
                    else:
                        # Cursor, erase, screen and mode changes do not prove that
                        # the text on either side occupied adjacent screen cells.
                        self.matched = 0
                elif len(self.parameters) <= 256:
                    self.parameters.append(byte)
                continue
            if self.escape in {"string", "string_escape"}:
                if byte == 0x07 or (self.escape == "string_escape" and byte == 0x5C):
                    self.escape = ""
                else:
                    self.escape = "string_escape" if byte == 0x1B else "string"
                continue
            if self.escape == "escape":
                if byte == 0x5B:
                    self.escape = "csi"
                    self.parameters.clear()
                elif byte in b"]PX^_":
                    self.matched = 0
                    self.escape = "string"
                else:
                    self.matched = 0
                    self.escape = ""
                continue
            if byte == 0x1B:
                self.escape = "escape"
                continue
            if self.concealed or not self.expected:
                self.matched = 0
                continue
            if byte == self.expected[self.matched]:
                self.matched += 1
                if self.matched == len(self.expected):
                    observed = True
                    self.matched = 0
            else:
                # Our canaries use an initial '~' that cannot recur later.
                self.matched = int(byte == self.expected[0])
        return observed


def _stop_cli(child):
    child.terminate()
    try:
        return child.wait(timeout=12)
    except subprocess.TimeoutExpired:
        child.kill()
        return child.wait(timeout=2)


def _leader(control_fd, command):
    # This helper owns the session and the CLI stays in its foreground group.
    # Keeping the leader alive until ACK avoids losing queued PTY bytes on macOS.
    with socket.socket(fileno=control_fd) as control:
        fcntl.ioctl(sys.stdin.fileno(), termios.TIOCSCTTY, 0)
        if os.tcgetpgrp(0) != os.getpgrp():
            return 1
        try:
            child = subprocess.Popen(command)
        except OSError:
            return 1
        try:
            while child.poll() is None:
                if select.select([control], [], [], 0.1)[0]:
                    if control.recv(1) != b"a":
                        _stop_cli(child)
                    break
            if child.poll() is None:
                _stop_cli(child)
            control.sendall(STATUS.pack(child.returncode))
            control.settimeout(CONTROL_WAIT)
            return 0 if control.recv(1) == b"a" else 1
        finally:
            if child.poll() is None:
                _stop_cli(child)


class PtySession:
    def __init__(self, master, control):
        self.master = master
        self.control = control
        self.status_bytes = bytearray()
        self.exit_status = None
        self.marker = Marker()
        self.receipt_observed = False
        self.responder = Responder((), monotonic())

    def pump(self, timeout, *, diagnostic_only=False):
        ready, _, _ = select.select([self.master, self.control], [], [], timeout)
        if self.control in ready and self.exit_status is None:
            batch = self.control.recv(STATUS.size - len(self.status_bytes))
            if not batch:
                raise PtyError("controlling_pty_closed")
            self.status_bytes.extend(batch)
            if len(self.status_bytes) == STATUS.size:
                self.exit_status = STATUS.unpack(self.status_bytes)[0]
        if self.master in ready:
            try:
                chunk = os.read(self.master, PTY_READ_SIZE)
                if not diagnostic_only and self.exit_status is None:
                    self.receipt_observed = self.marker.feed(chunk) or self.receipt_observed
                self.responder.observe(
                    chunk, monotonic(), allow_answers=self.exit_status is None and not diagnostic_only,
                )
                return chunk
            except BlockingIOError:
                pass
        return b""

    def wait_receipt(self, receipt, deadline):
        self.marker.expect(receipt)
        self.receipt_observed = False
        while self.exit_status is None and monotonic() < deadline:
            wake = deadline
            if self.responder.pending is not None:
                wake = min(deadline, self.responder.pending[1] + SETTLE)
            self.pump(max(0, wake - monotonic()))
            if self.receipt_observed:
                return
            if self.exit_status is None and self.responder.due(monotonic()):
                self.answer(self.responder.take(monotonic()), deadline)
                if self.receipt_observed:
                    return
        if self.exit_status is not None:
            raise PtyError("cli_exited_before_receipt")
        raise PtyError("receipt_deadline")

    def answer(self, item, deadline):
        """Send one fixed answer, one key at a time, still reading the screen."""
        for name in item.keys:
            self.responder.note(f"key:{name}")
            self.write(KEYS[name], deadline)
            pause = min(deadline, monotonic() + KEY_GAP)
            while self.exit_status is None and monotonic() < pause:
                self.pump(max(0, pause - monotonic()))
            if self.exit_status is not None:
                return
        self.responder.answered_at[item.name] = monotonic()

    def wait_quiet(self, deadline):
        """Pace input after terminal output. Silence does not prove composer readiness."""
        deadline = min(deadline, monotonic() + QUIET_LIMIT)
        after = monotonic() + QUIET_WINDOW
        while self.exit_status is None and monotonic() < deadline:
            if self.pump(max(0, min(after, deadline) - monotonic())):
                after = monotonic() + QUIET_WINDOW
            if monotonic() >= after:
                return
        if self.exit_status is not None:
            raise PtyError("cli_exited_before_quiet_window")
        raise PtyError("quiet_window_not_observed")

    def submit(self, text, deadline):
        if self.exit_status is not None or monotonic() >= deadline:
            raise PtyError("submit_after_exit_or_deadline")
        payload = text.encode("ascii")
        if len(payload) > 512 or not all(32 <= byte < 127 for byte in payload):
            raise PtyError("invalid_pty_input")
        self.write(payload, deadline)
        cr_time = monotonic() + SUBMIT_PAUSE
        while self.exit_status is None and monotonic() < min(cr_time, deadline):
            self.pump(max(0, min(cr_time, deadline) - monotonic()))
        if self.exit_status is not None or monotonic() >= deadline:
            raise PtyError("submit_after_exit_or_deadline")
        self.write(b"\r", deadline)

    def write(self, payload, deadline):
        while payload:
            remaining = deadline - monotonic()
            if remaining <= 0 or not select.select([], [self.master], [], remaining)[1]:
                raise PtyError("pty_input_deadline")
            try:
                count = os.write(self.master, payload)
            except BlockingIOError:
                continue
            if count <= 0:
                raise PtyError("pty_input_closed")
            payload = payload[count:]

    def wait_exit(self, deadline):
        while self.exit_status is None and monotonic() < deadline:
            self.pump(max(0, deadline - monotonic()))
        if self.exit_status is None:
            raise PtyError("close_deadline")

    def drain(self):
        # Read a bounded queued tail before the session leader revokes the PTY.
        for _ in range(64):
            if not self.pump(0, diagnostic_only=True):
                return
        raise PtyError("pty_drain_limit")


def _restored(slave, original):
    current = termios.tcgetattr(slave)
    pending_input = getattr(termios, "PENDIN", 0) if sys.platform == "darwin" else 0
    current[3] &= ~pending_input
    original = original.copy()
    original[3] &= ~pending_input
    return current == original


def drive(command, first_receipt, submission, second_receipt, *, timeout, env, cwd, screens=()):
    """Run the CLI once. Return only status facts, after closing the owned PTY."""
    if any(
        not receipt.startswith("~") or "~" in receipt[1:] or not receipt.isascii()
        for receipt in (first_receipt, second_receipt)
    ) or first_receipt == second_receipt or second_receipt in submission:
        raise ValueError("receipt canaries must be distinct, one-shot ASCII markers")
    master, slave = pty.openpty()
    original = termios.tcgetattr(slave)
    leader = None
    responder = None
    stage = "startup"
    reason = None
    receipts = 0
    restored = False
    exit_status = None
    try:
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        os.set_blocking(master, False)
        parent, control = socket.socketpair()
        with parent, control:
            leader = subprocess.Popen(
                [sys.executable, str(Path(__file__).resolve()), "--leader", str(control.fileno()),
                 *command],
                cwd=cwd, env=env, start_new_session=True, pass_fds=(control.fileno(),),
                stdin=slave, stdout=slave, stderr=slave,
            )
            control.close()
            session = PtySession(master, parent)
            session.responder = responder = Responder(screens, monotonic())
            deadline = monotonic() + timeout
            try:
                stage = "first_receipt"
                session.wait_receipt(first_receipt, deadline)
                receipts = 1
                responder.note("receipt:1")
                stage = "second_submission"
                session.wait_quiet(deadline)
                session.submit(submission, deadline)
                responder.note("submit:follow_up")
                stage = "second_receipt"
                session.wait_receipt(second_receipt, deadline)
                receipts = 2
                responder.note("receipt:2")
                stage = "native_exit"
                session.wait_quiet(deadline)
                session.submit("/exit", deadline)
                responder.note("submit:exit")
                session.wait_exit(deadline)
                responder.activity(monotonic(), final=True)
                responder.note(f"exit:observed:total={responder.total_bytes}B")
                stage = "drain"
                session.drain()
                stage = "complete"
            except (OSError, PtyError, KeyboardInterrupt) as error:
                if isinstance(error, KeyboardInterrupt):
                    stage = "operator_interrupt"
                    reason = "operator_interrupt"
                else:
                    reason = str(error) if isinstance(error, PtyError) else "terminal_io_error"
                responder.activity(monotonic(), final=True)
                responder.note(f"stop:{stage}:{reason}:total={responder.total_bytes}B")
                if session.exit_status is None:
                    try:
                        parent.sendall(b"x")
                        session.wait_exit(monotonic() + 45)
                    except (OSError, PtyError):
                        pass
            exit_status = session.exit_status
            if reason is not None and exit_status is not None and stage != "drain":
                # The status can arrive before all the child's output. Observe its
                # queued screens while the leader still owns the terminal.
                try:
                    session.drain()
                except (OSError, PtyError):
                    responder.note("diagnostic:pty_tail_incomplete")
                responder.activity(monotonic(), final=True)
            restored = exit_status is not None and _restored(slave, original)
            if exit_status is not None:
                if not restored:
                    termios.tcsetattr(slave, termios.TCSANOW, original)
                parent.sendall(b"a")
                try:
                    leader_status = leader.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    leader_status = None
                if leader_status != 0:
                    stage = "leader_exit"
                    reason = "controlling_pty_leader_failed"
    finally:
        try:
            if leader is not None and leader.poll() is None:
                try:
                    termios.tcsetattr(slave, termios.TCSANOW, original)
                except termios.error:
                    # An unexpected leader exit may already have revoked this PTY.
                    pass
                try:
                    # Only the new PTY's session owns this group.
                    os.killpg(leader.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                leader.wait(timeout=2)
        finally:
            os.close(slave)
            os.close(master)
    if stage == "complete" and not restored:
        stage = "terminal_restoration"
        reason = "terminal_modes_not_restored"
    if stage == "complete" and exit_status != 0:
        stage = "cli_exit"
        reason = "cli_nonzero_exit"
    timeline = tuple(responder.timeline) if responder is not None else ()
    return InteractiveProof(stage, receipts, exit_status, restored, reason, timeline)


def provider_turns():
    """Return the initial prompt, receipts and follow-up for an ordinary conversation."""
    first = uuid4().hex[:12].upper()
    second = uuid4().hex[:12].upper()
    prompt = (
        "Reply with a single ASCII string. Begin with a tilde, then write "
        f"{first[:6]} followed immediately by {first[6:]} with no spaces. "
        "End with an exclamation mark. Remember the characters between the punctuation "
        "for my next message. Do not use tools or change files."
    )
    follow_up = (
        "Reply with a single ASCII string. Begin with the tilde and the block of characters "
        "from your last answer, without its final exclamation mark. Then write a plus sign "
        f"and {second[:6]} followed immediately by {second[6:]} without spaces. "
        "End with an exclamation mark. Do not use tools or change files."
    )
    return prompt, "~" + first + "!", follow_up, "~" + first + "+" + second + "!"


def run_provider(command, *, timeout, env, cwd, client=None):
    """Start the managed CLI directly, with its positional first interactive prompt."""
    if "--prompt" in command or "--interactive" in command or "--" in command:
        raise ValueError("pass the managed CLI options without transport or positional arguments")
    prompt, first, follow_up, second = provider_turns()
    if first in prompt or first in follow_up or second in prompt or second in follow_up:
        raise ValueError("a terminal receipt appears literally in the entered prompts")
    return drive(
        [*command, "--interactive", "--prompt", prompt], first, follow_up, second,
        timeout=timeout, env=env, cwd=cwd, screens=SCREENS.get(client, ()),
    )


# Startup screens drawn like Claude Code 2.1.329 (words placed by cursor moves) and Codex 0.170.0,
# each with the exact answer bytes that must follow it. An empty answer means "no key at all".
STARTUP_FIXTURES = {
    "claude": (
        (b"\x1b[2GLet's\x1b[8Gget\x1b[12Gstarted.\r\n\x1b[2G\xe2\x9d\xaf Dark mode\r\n", b"\r"),
        (b"\x1b[38;5;220mDetected\x1b[11Ga\x1b[13Gcustom\x1b[20GAPI\x1b[24Gkey\x1b[28Gin\x1b[31Gyour"
         b"\x1b[36Genvironment\x1b[39m\r\n\x1b[2GYes\r\n\x1b[2G\xe2\x9d\xaf No (recommended)\r\n", b"\x1b[A\r"),
        (b"\x1b[2G\x1b[1mSecurity\x1b[11Gnotes:\x1b[22m\r\n", b"\r"),
        (b"\x1b[2GQuick\x1b[8Gsafety\x1b[15Gcheck:\x1b[22GIs\r\n\x1b[2G\xe2\x9d\xaf No, exit\r\n"
         b"\x1b[4GYes, I trust this folder\r\n", b"\x1b[B\r"),
    ),
    "codex": ((b"\x1b[2GCodex\x1b[8Gjust\x1b[13Ggot\x1b[17Gan\x1b[20Gupgrade.\r\n", b""),),
}


def _startup_fixture(name):
    import tty

    previous = termios.tcgetattr(0)
    tty.setraw(0)
    try:
        for drawn, expected in STARTUP_FIXTURES[name]:
            os.write(1, drawn)
            shown = monotonic()
            if not expected:
                if select.select([0], [], [], 2 * SETTLE)[0]:
                    return 31
                continue
            received = b""
            while len(received) < len(expected):
                if not select.select([0], [], [], 20)[0]:
                    return 32
                if not received and monotonic() - shown < 1.0:
                    return 33
                received += os.read(0, 64)
            if received != expected:
                return 34
        for receipt, line in ((b"~MARKER1", b"next turn\r"), (b"~MARKER2", b"/exit\r")):
            os.write(1, receipt + b"\r\n")
            data = b""
            while not data.endswith(b"\r"):
                data += os.read(0, 128)
            if data != line:
                return 35
        return 0
    finally:
        termios.tcsetattr(0, termios.TCSANOW, previous)


def self_test():
    from unittest.mock import patch

    receipt = "~MARKER1"
    if Marker(receipt).feed(b"~MAR\x1b[1CKER1"):
        raise PtyError("a column move falsely joined two receipt fragments")
    if Marker(receipt).feed(b"~MAR\x1b[?1049hKER1"):
        raise PtyError("an alternate-screen switch falsely joined two receipt fragments")
    scanner = Marker(receipt)
    if not scanner.feed(b"~MARKER1\x1b[8"):
        raise PtyError("a complete receipt was lost before a split escape sequence")
    scanner.expect("~MARKER2")
    if scanner.feed(b"m\x1b[48;2;28;0;8m~MARKER2"):
        raise PtyError("a concealed receipt passed after a phase change or color command")
    if not scanner.feed(b"\x1b[28m~MAR\x1b[38;5;8mKER2"):
        raise PtyError("a visible receipt was lost after clearing concealment")
    command = [sys.executable, "-c", """
import os
import select
import sys
import termios
import time
import tty

assert all(os.isatty(fd) for fd in (0, 1, 2))
with open('/dev/tty', 'rb') as terminal:
    assert os.tcgetpgrp(terminal.fileno()) == os.getpgrp()
import fcntl
import struct
assert struct.unpack('HHHH', fcntl.ioctl(0, termios.TIOCGWINSZ, b'\\0' * 8))[:2] == (40, 120)

def line():
    data = b''
    while not data.endswith(b'\\r'):
        data += os.read(0, 128)
    return data

previous = termios.tcgetattr(0)
tty.setraw(0)
try:
    if sys.argv[1:] == ['concealed']:
        os.write(1, b'~MAR\\x1b[8mKER\\x1b[28m1\\r\\n')
        if not select.select([0], [], [], 2)[0]:
            sys.exit(24)
    else:
        os.write(1, b'~MAR')
        os.write(1, b'\\x1b[')
        time.sleep(0.1)
        os.write(1, b'1mKER1\\x1b[0m\\r\\n')
    assert line() == b'next turn\\r'
    if sys.argv[1:] == ['busy']:
        # The fixture's composer never became ready. It received the bytes
        # but rejected them instead of enqueuing a turn.
        sys.exit(23)
    os.write(1, b'~MARKER2\\r\\n')
    assert line() == b'/exit\\r'
finally:
    termios.tcsetattr(0, termios.TCSANOW, previous)
"""]
    proof = drive(
        command, "~MARKER1", "next turn", "~MARKER2",
        timeout=10, env=os.environ.copy(), cwd=Path.cwd(),
    )
    if not proof.passed:
        raise PtyError(f"isolated PTY self-test failed at {proof.stage}")
    concealed = drive(
        [*command, "concealed"], "~MARKER1", "next turn", "~MARKER2",
        timeout=10, env=os.environ.copy(), cwd=Path.cwd(),
    )
    if not (
        not concealed.passed and concealed.stage == "first_receipt"
        and concealed.receipts == 0 and concealed.cli_exit_status == 24
        and concealed.reason == "cli_exited_before_receipt" and concealed.termios_restored
    ):
        raise PtyError("a concealed receipt was counted in the controlling PTY")
    busy = drive(
        [*command, "busy"], "~MARKER1", "next turn", "~MARKER2",
        timeout=10, env=os.environ.copy(), cwd=Path.cwd(),
    )
    if not (
        not busy.passed and busy.stage == "second_receipt"
        and busy.receipts == 1 and busy.cli_exit_status == 23
        and busy.reason == "cli_exited_before_receipt" and busy.termios_restored
    ):
        raise PtyError("a write to the PTY did not remain distinct from composer acceptance")
    echo_only = [
        sys.executable, "-c",
        "import os,time; os.write(1,b'~MAR KER1\\r\\n\\x1b]~MARKER1\\x07'); time.sleep(20)",
    ]
    stopped = drive(
        echo_only, "~MARKER1", "next turn", "~MARKER2",
        timeout=1, env=os.environ.copy(), cwd=Path.cwd(),
    )
    if not (
        not stopped.passed and stopped.stage == "first_receipt"
        and stopped.receipts == 0 and stopped.cli_exit_status == -signal.SIGTERM
        and stopped.termios_restored and stopped.reason == "receipt_deadline"
    ):
        raise PtyError("echo or hidden text wrongly passed, or bounded termination was not observed")
    # Put the terminal status ahead of the first short PTY read. The fake child's
    # fixed screens and its would-be receipt then arrive only on later reads.
    wait_receipt = PtySession.wait_receipt

    def wait_for_terminal(session, receipt, deadline):
        if not select.select([session.control], [], [], max(0, deadline - monotonic()))[0]:
            raise AssertionError("terminal-tail fixture did not announce its exit")
        wait_receipt(session, receipt, deadline)

    late_output = [
        sys.executable, "-c",
        "import os; os.write(1, b'~MARKER1\\r\\n' + b'.' * 256 + "
        'b"\\r\\nLet\'s get started.\\r\\nInvalid API key\\r\\n~MARKER1\\r\\n"); raise SystemExit(1)',
    ]
    with (
        patch.object(PtySession, "wait_receipt", wait_for_terminal),
        patch.dict(globals(), PTY_READ_SIZE=32),
    ):
        terminal_tail = drive(
            late_output, "~MARKER1", "next turn", "~MARKER2",
            timeout=10, env=os.environ.copy(), cwd=Path.cwd(), screens=CLAUDE_SCREENS,
        )
    events = [entry.split(" ", 1)[1] for entry in terminal_tail.timeline]
    if not (
        not terminal_tail.passed and terminal_tail.stage == "first_receipt"
        and terminal_tail.receipts == 0 and terminal_tail.cli_exit_status == 1
        and terminal_tail.reason == "cli_exited_before_receipt" and terminal_tail.termios_restored
        and {"screen:claude.theme", "screen:claude.invalid_api_key"}.issubset(events)
        and not any(event.startswith(("key:", "receipt:")) for event in events)
    ):
        raise PtyError("terminal-tail screens were lost or changed the failed attempt")
    expected = {
        "claude": ["screen:claude.theme", "key:enter", "screen:claude.api_key", "key:up", "key:enter",
                   "screen:claude.security_notes", "key:enter", "screen:claude.workspace_trust", "key:down",
                   "key:enter", "receipt:1"],
        "codex": ["screen:codex.model_migration", "receipt:1"],
    }
    for fixture, screens in (("claude", CLAUDE_SCREENS), ("codex", CODEX_SCREENS)):
        started = drive(
            [sys.executable, str(Path(__file__).resolve()), "--startup-fixture", fixture],
            "~MARKER1", "next turn", "~MARKER2",
            timeout=90, env=os.environ.copy(), cwd=Path.cwd(), screens=screens,
        )
        events = [entry.split(" ", 1)[1] for entry in started.timeline]
        ordered = [event for event in events if event.startswith(("screen:", "key:", "receipt:1"))]
        if not started.passed or ordered[:len(expected[fixture])] != expected[fixture]:
            raise PtyError(f"startup screens {fixture} were not answered exactly at {started.stage} "
                           f"(exit {started.cli_exit_status})")
    print("PASS controlling PTY, two distinct receipts, native exit, restored termios, startup and terminal screens")


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "--self-test":
        self_test()
    elif len(sys.argv) == 3 and sys.argv[1] == "--startup-fixture" and sys.argv[2] in STARTUP_FIXTURES:
        sys.exit(_startup_fixture(sys.argv[2]))
    elif len(sys.argv) > 3 and sys.argv[1] == "--leader":
        try:
            sys.exit(_leader(int(sys.argv[2]), sys.argv[3:]))
        except (OSError, subprocess.TimeoutExpired):
            sys.exit(1)
    else:
        sys.exit("Use --self-test or import the driver from the E2E recipe.")
