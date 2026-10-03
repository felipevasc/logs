"""Linux supervisor: adopt orphaned descendants and reap only this child's tree.

Requires Python 3.9+ and Linux pidfds (Ubuntu 22.04 CI provides both). A browser
may create a separate process group, so killing only the Node group is unsafe.
"""
import ctypes
import os
import select
import signal
import subprocess
import sys
import time


def children(pid):
    try:
        with open(f"/proc/{pid}/task/{pid}/children", encoding="ascii") as handle:
            return [int(value) for value in handle.read().split()]
    except FileNotFoundError:
        return []


def freeze_tree(pid, parent, owned):
    try:
        handle = os.pidfd_open(pid)
    except ProcessLookupError:
        return
    try:
        with open(f"/proc/{pid}/status", encoding="ascii") as status:
            ppid = next(int(line.split()[1]) for line in status if line.startswith("PPid:"))
        if ppid != parent:
            return
        # The pidfd pins this process identity even if its numeric PID is reused.
        signal.pidfd_send_signal(handle, signal.SIGSTOP)
        for child in children(pid):
            freeze_tree(child, pid, owned)
        owned.append(handle)
        handle = None
    except (FileNotFoundError, ProcessLookupError):
        pass
    finally:
        if handle is not None:
            os.close(handle)


def cleanup():
    deadline = time.monotonic() + 10
    while children(os.getpid()):
        owned = []
        try:
            for pid in children(os.getpid()):
                freeze_tree(pid, os.getpid(), owned)
            for handle in owned:
                try:
                    signal.pidfd_send_signal(handle, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        finally:
            for handle in owned:
                os.close(handle)
        while True:
            try:
                if os.waitpid(-1, os.WNOHANG)[0] == 0:
                    break
            except ChildProcessError:
                break
        if time.monotonic() >= deadline:
            raise RuntimeError("Owned process tree did not exit")
        time.sleep(0.01)


def run():
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise RuntimeError("Preview supervision requires Python 3.9+ with Linux pidfd support")
    try:
        probe = os.pidfd_open(os.getpid())
        os.close(probe)
    except OSError as error:
        raise RuntimeError("Preview supervision requires Linux kernel pidfd support (Ubuntu 22.04 or newer is supported)") from error
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "Cannot become a child subreaper")
    stopped = False

    def stop(_signal, _frame):
        nonlocal stopped
        stopped = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    child = subprocess.Popen(sys.argv[1:], stdin=subprocess.DEVNULL, start_new_session=True)
    try:
        while child.poll() is None:
            if stopped or select.select([sys.stdin], [], [], 0.05)[0]:
                return 130
        return child.returncode if child.returncode >= 0 else 128 - child.returncode
    finally:
        cleanup()


try:
    sys.exit(run())
except Exception as error:
    print(f"Process supervisor failed: {error}", file=sys.stderr)
    sys.exit(125)
