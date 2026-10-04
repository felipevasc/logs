import io, json, os, stat, sys, tarfile

# Executed as a fixed OpenSSH command. All user input is JSON on stdin.
request = json.load(sys.stdin)
files, skipped, seen, total, directories = [], [], set(), 0, 0
def admit(path):
    global total
    info = os.lstat(path)
    if not stat.S_ISREG(info.st_mode) or path in seen:
        return
    seen.add(path)
    total += info.st_size
    if len(files) >= request['maxFiles'] or total > request['maxBytes']:
        raise RuntimeError('Selected files exceed the collection limit; choose a smaller path.')
    files.append((path, info))
for path in request['paths']:
    if not os.path.exists(path):
        skipped.append(path)
        continue
    if os.path.islink(path):
        raise RuntimeError('Symbolic links cannot be collected: ' + path)
    if os.path.isdir(path):
        pending = [path]
        while pending:
            directory = pending.pop()
            directories += 1
            if directories > 8192:
                raise RuntimeError('Too many directories; select a narrower path.')
            with os.scandir(directory) as entries:
                for entry in entries:
                    if entry.is_symlink():
                        continue
                    if entry.is_dir(follow_symlinks=False):
                        if len(pending) >= 8192:
                            raise RuntimeError('Too many directories; select a narrower path.')
                        pending.append(entry.path)
                    else:
                        admit(entry.path)
    else:
        admit(path)
if not files:
    raise RuntimeError('No regular files were found in the selected paths.')
files.sort(key=lambda item: item[0])
if request['test']:
    for path, info in files:
        with open(path, 'rb') as stream:
            stream.read(1)
    print(json.dumps({'files': len(files), 'bytes': total, 'skipped': skipped}))
else:
    with tarfile.open(fileobj=sys.stdout.buffer, mode='w|') as archive:
        metadata = json.dumps({'skipped': skipped, 'sources': [p for p, _ in files]}).encode()
        entry = tarfile.TarInfo('manifest.json'); entry.size = len(metadata)
        archive.addfile(entry, io.BytesIO(metadata))
        for number, (path, info) in enumerate(files):
            descriptor = os.open(path, os.O_RDONLY | getattr(os, 'O_NOFOLLOW', 0) | getattr(os, 'O_NONBLOCK', 0))
            with os.fdopen(descriptor, 'rb') as stream:
                current = os.fstat(stream.fileno())
                if not stat.S_ISREG(current.st_mode) or current.st_ino != info.st_ino or current.st_dev != info.st_dev or current.st_size < info.st_size:
                    raise RuntimeError('Source changed during acquisition: ' + path)
                entry = tarfile.TarInfo('files/%04d/%s' % (number, os.path.basename(path)))
                entry.size = info.st_size; entry.mtime = info.st_mtime
                archive.addfile(entry, stream)
