"""Nautilus (GNOME Files) context menu for Rcomp.

Adds "Compress with Rcomp" for any selection and "Decompress with Rcomp" when
every selected item is a supported archive. Loaded by nautilus-python from
/usr/share/nautilus-python/extensions/; works with the Nautilus 3 and 4 APIs.
"""

import subprocess

from gi.repository import GObject, Nautilus

ARCHIVE_SUFFIXES = (
    ".zip", ".7z", ".rar", ".tar", ".gz", ".tgz", ".bz2", ".tbz2",
    ".xz", ".txz", ".zst", ".tzst", ".lz4", ".br",
)


class RcompMenuProvider(GObject.GObject, Nautilus.MenuProvider):
    def get_file_items(self, *args):
        # Nautilus 3 passes (window, files); Nautilus 4 passes (files,).
        files = args[-1]
        paths = []
        for f in files:
            location = f.get_location()
            path = location.get_path() if location is not None else None
            if path is None:
                return []  # Remote locations are not supported.
            paths.append(path)
        if not paths:
            return []

        items = [self._item("Compress", "Compress with Rcomp", "--compress", paths)]
        if all(p.lower().endswith(ARCHIVE_SUFFIXES) for p in paths):
            items.append(
                self._item("Decompress", "Decompress with Rcomp", "--decompress", paths)
            )
        return items

    def _item(self, name, label, flag, paths):
        item = Nautilus.MenuItem(name="Rcomp::" + name, label=label)
        item.connect("activate", self._launch, flag, paths)
        return item

    @staticmethod
    def _launch(_item, flag, paths):
        subprocess.Popen(["Rcomp", flag, *paths], start_new_session=True)
