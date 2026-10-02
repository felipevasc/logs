"""Extract flat release archives without trusting their paths, types, sizes or duplicate names."""
import os
import re
import stat
import sys
import zipfile


def unpack(archive, destination):
    with zipfile.ZipFile(archive) as bundle:
        entries = bundle.infolist()
        if not 5 <= len(entries) <= 7:
            raise ValueError('Unexpected artifact entry count')
        names = set()
        total = 0
        for entry in entries:
            name = entry.filename
            if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._+-]*', name) or name.lower() in names:
                raise ValueError('Unsafe or duplicate artifact path')
            names.add(name.lower())
            mode = entry.external_attr >> 16
            if stat.S_IFMT(mode) not in (0, stat.S_IFREG) or entry.is_dir() or entry.flag_bits & 1:
                raise ValueError('Only unencrypted regular files are accepted')
            if entry.compress_type not in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED):
                raise ValueError('Unsupported archive compression')
            if not 0 < entry.file_size <= 1024 * 1024 * 1024:
                raise ValueError('Invalid or oversized artifact entry')
            total += entry.file_size
        if total > 3 * 1024 * 1024 * 1024:
            raise ValueError('Artifact expansion limit exceeded')
        os.mkdir(destination)
        for entry in entries:
            # No extractall: write only previously validated flat filenames with exclusive creation.
            with bundle.open(entry) as source, open(os.path.join(destination, entry.filename), 'xb') as target:
                remaining = entry.file_size
                while remaining:
                    chunk = source.read(min(1024 * 1024, remaining))
                    if not chunk:
                        raise ValueError('Truncated artifact entry')
                    target.write(chunk)
                    remaining -= len(chunk)
                if source.read(1):
                    raise ValueError('Oversized artifact entry')


if __name__ == '__main__':
    unpack(sys.argv[1], sys.argv[2])
