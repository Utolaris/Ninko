"""Real PTY regression: uv run python tests/terminal.py [binary]. No live Clash calls."""
import errno
import http.server
import json
import os
import pty
import re
import select
import signal
import struct
import sys
import tempfile
import termios
import threading
import time
from urllib.parse import unquote
import fcntl
from pathlib import Path

CAPTURED = bytearray()


class Controller(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.path == "/providers/proxies":
            body = {"providers": {}}
        elif self.path == "/proxies":
            body = {"proxies": {
                name: {"name": name, "type": "Selector", "all": ["香港一", "香港二", "香港三", "香港四", "香港五", "香港六"], "now": "香港一"}
                for name in ["AI 专用", "其他"]
            }}
        elif "/delay?" in self.path:
            name = unquote(self.path.split("/")[2])
            pause, delay = {"香港一": (0.4, 123), "香港二": (1.5, 401), "香港三": (2, 1001), "香港四": (2.5, 250), "香港五": (4, 300), "香港六": (0.3, 60)}[name]
            time.sleep(pause)
            body = {"delay": delay}
        else:
            self.send_error(404)
            return
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass


def expect(fd, text, also=None):
    data = b""
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if select.select([fd], [], [], 0.1)[0]:
            try:
                chunk = os.read(fd, 65536)
                CAPTURED.extend(chunk)
                data += chunk
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                break
            plain = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", data.decode(errors="replace"))
            if text in plain and (also is None or also in plain):
                return plain
    raise AssertionError(f"未看到 {text!r}，终端输出：{data!r}")


def expect_result(fd, text):
    # A previous read may include several completed latency updates in one batch.
    plain = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", CAPTURED.decode(errors="replace"))
    return plain if text in plain else expect(fd, text)


def run(binary, address, directory):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.environ.pop("NO_COLOR", None)
        os.environ["CLICOLOR_FORCE"] = "1"
        for key in ("CLASH_SOCK", "CLASH_API", "CLASH_SECRET"):
            os.environ.pop(key, None)
        os.execv(binary, [binary, "--api", address, "--data-dir", directory])
    try:
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
        initial = expect(fd, "Merge.yaml")
        assert "全局配置" in initial, initial
        assert b"\x1b[38;5;183m" in CAPTURED
        # Both common arrow encodings must move selection, including wraparound.
        for key, label in [(b"\x1bOA", "退出"), (b"\x1bOB", "切换节点"),
                           (b"\x1b[B", "查看订阅"), (b"\x1b[A", "切换节点")]:
            os.write(fd, key)
            expect(fd, f"❯ {label}")
        os.write(fd, b"\r")
        expect(fd, "选择策略组")
        os.write(fd, "其".encode())
        expect(fd, "❯ 其他")
        os.write(fd, b"\x7f")
        expect(fd, "❯ AI 专用")
        os.write(fd, b"\x1bOB")
        expect(fd, "❯ 其他")
        os.write(fd, b"\r")
        expect(fd, "❯ 香港一  ← 当前使用")
        os.write(fd, "香港".encode())
        expect(fd, "搜索：香港")
        captured_before = len(CAPTURED)
        os.write(fd, b"\x1b[B")
        expect(fd, "❯ 香港二", also="香港六")
        current_lines = [line for line in CAPTURED[captured_before:].decode().splitlines() if "香港一" in line and "当前使用" in line]
        assert current_lines and all("38;5;183m" in line for line in current_lines), current_lines
        os.write(fd, b"\x1bOA")
        expect(fd, "❯ 香港一  ← 当前使用")
        partial = expect(fd, "123 ms")
        assert "香港二  测速中" in partial, partial
        os.write(fd, b"\x1b[B")
        expect(fd, "❯ 香港二")
        os.write(fd, b"\x1b[A")
        expect(fd, "❯ 香港一")
        expect_result(fd, "401 ms")
        expect_result(fd, "香港三  超时")
        expect_result(fd, "香港四  250 ms")
        previous_query = "香港"
        for query in ["xiang", "xianggang", "xg", "XIANGGANG"]:
            os.write(fd, b"\x7f" * len(previous_query) + query.encode())
            pinyin_result = expect(fd, f"搜索：{query}", also="香港五")
            assert all(name in pinyin_result for name in ["香港一", "香港二", "香港三", "香港四", "香港五"]), pinyin_result
            previous_query = query
        for color in (1, 2, 4, 208):
            assert f"\x1b[38;5;{color}m".encode() in CAPTURED, f"没有显示颜色 {color}"
        cancelled_at = time.monotonic()
        os.write(fd, b"\x1b")
        returned = expect(fd, "❯ 切换节点")
        assert time.monotonic() - cancelled_at < 1
        assert "提示" not in returned and "按任意键" not in returned
        os.write(fd, b"\x1b")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            # Keep draining the PTY while the app restores terminal modes on exit.
            if select.select([fd], [], [], 0.02)[0]:
                try:
                    os.read(fd, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
            child, status = os.waitpid(pid, os.WNOHANG)
            if child:
                assert os.waitstatus_to_exitcode(status) == 0
                pid = None
                print("PTY passed: arrows, search, immediate latency refresh, timeout, silent Esc while testing")
                return
            time.sleep(0.02)
        raise AssertionError("Esc 没有退出")
    finally:
        os.close(fd)
        if pid:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)


if __name__ == "__main__":
    binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/ninko").resolve())
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / "profiles").mkdir()
        (root / "profiles" / "Merge.yaml").write_text("# Clash\n")
        (root / "profiles.yaml").write_text("items:\n- uid: Merge\n  type: merge\n  file: Merge.yaml\n")
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Controller)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            run(binary, f"http://127.0.0.1:{server.server_port}", directory)
        finally:
            server.shutdown()
