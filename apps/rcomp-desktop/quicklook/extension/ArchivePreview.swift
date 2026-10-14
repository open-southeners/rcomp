// Renders an archive's contents as the HTML page shown by Quick Look.
//
// The listing comes from rcomp-core through the rcomp-quicklook static library
// (see include/rcomp_quicklook.h). The page mirrors a Finder list view: a
// summary line, a "Show hidden items" toggle, and a collapsible tree of
// folders and files with sizes. Quick Look runs previews without JavaScript,
// so collapsing uses <details> and the toggle is a CSS-only checkbox.

import Foundation

/// The JSON document returned by `rcomp_ql_list_json`.
struct Listing: Decodable {
    struct Entry: Decodable {
        let path: String
        let size: UInt64
        let is_dir: Bool
    }

    let ok: Bool
    let format: String?
    let entries: [Entry]?
    let kind: String?
    let message: String?
}

/// One folder or file in the rendered tree.
final class Node {
    let name: String
    var isDir: Bool
    var size: UInt64 = 0
    var children: [String: Node] = [:]

    init(name: String, isDir: Bool) {
        self.name = name
        self.isDir = isDir
    }

    /// Folders first, then names in Finder order.
    var sortedChildren: [Node] {
        children.values.sorted {
            if $0.isDir != $1.isDir { return $0.isDir }
            return $0.name.localizedStandardCompare($1.name) == .orderedAscending
        }
    }

    var isHidden: Bool { name.hasPrefix(".") || name == "__MACOSX" }
}

enum ArchivePreview {
    /// Rows rendered at most; larger archives end with a "… more" note.
    static let rowLimit = 5_000

    /// Ask the static library for the archive's listing.
    static func list(_ url: URL) -> Listing {
        guard let raw = rcomp_ql_list_json(url.path) else {
            return Listing(ok: false, format: nil, entries: nil, kind: "error", message: "No listing.")
        }
        defer { rcomp_ql_free(raw) }
        let data = Data(bytes: raw, count: strlen(raw))
        if let listing = try? JSONDecoder().decode(Listing.self, from: data) {
            return listing
        }
        return Listing(ok: false, format: nil, entries: nil, kind: "error", message: "Unreadable listing.")
    }

    /// Build a folder tree from flat entry paths. Folders that only exist
    /// implicitly (as a prefix of a file path) are created along the way.
    static func tree(_ entries: [Listing.Entry]) -> Node {
        let root = Node(name: "", isDir: true)
        for entry in entries {
            let parts = entry.path
                .split(separator: "/", omittingEmptySubsequences: true)
                .map(String.init)
                .filter { $0 != "." }
            guard !parts.isEmpty else { continue }
            var node = root
            for (i, part) in parts.enumerated() {
                let last = i == parts.count - 1
                if let child = node.children[part] {
                    if last && entry.is_dir { child.isDir = true }
                    node = child
                } else {
                    let child = Node(name: part, isDir: !last || entry.is_dir)
                    node.children[part] = child
                    node = child
                }
            }
            if !entry.is_dir { node.size = entry.size }
        }
        return root
    }

    /// The complete HTML page for `url`.
    static func render(fileURL url: URL) -> String {
        let listing = list(url)
        guard listing.ok, let entries = listing.entries else {
            return page(body: message(for: listing, url: url))
        }
        let root = tree(entries)
        var files = 0
        var folders = 0
        var total: UInt64 = 0
        count(root, files: &files, folders: &folders, total: &total)

        var rows = ""
        var rendered = 0
        // A single top-level folder is opened, like Finder's first level.
        let expandFirst = root.children.count == 1
        for child in root.sortedChildren {
            rows += row(child, open: expandFirst, rendered: &rendered)
        }
        if rendered >= rowLimit {
            rows += "<p class=\"more\">Only the first \(rowLimit) items are shown.</p>"
        }

        let items = files + folders
        let summary = "\(items) item\(items == 1 ? "" : "s"), uncompressed size: \(bytes(total))"
        let format = escape(listing.format ?? "")
        return page(body: """
            <input type="checkbox" id="hidden" class="toggle">
            <header>
              <span>\(summary)</span>
              <span class="format">\(format)</span>
              <label for="hidden">Show hidden items</label>
            </header>
            <div class="columns"><span>Name</span><span>Size</span></div>
            <main>\(rows.isEmpty ? "<p class=\"empty\">This archive is empty.</p>" : rows)</main>
            """)
    }

    /// Count visible items (hidden ones are collapsed by default, so the
    /// summary matches what is listed) and sum every file's size.
    private static func count(
        _ node: Node, files: inout Int, folders: inout Int, total: inout UInt64, visible: Bool = true
    ) {
        for child in node.children.values {
            let shown = visible && !child.isHidden
            if child.isDir {
                if shown { folders += 1 }
                count(child, files: &files, folders: &folders, total: &total, visible: shown)
            } else {
                if shown { files += 1 }
                total += child.size
            }
        }
    }

    private static func row(_ node: Node, open: Bool, rendered: inout Int) -> String {
        guard rendered < rowLimit else { return "" }
        rendered += 1
        let hidden = node.isHidden ? " hidden-item" : ""
        let name = escape(node.name)
        if !node.isDir {
            return """
                <div class="row\(hidden)"><span class="name"><i class="file"></i>\(name)</span>\
                <span class="size">\(bytes(node.size))</span></div>
                """
        }
        var inner = ""
        for child in node.sortedChildren {
            inner += row(child, open: false, rendered: &rendered)
        }
        return """
            <details class="\(hidden)"\(open ? " open" : "")><summary class="row">\
            <span class="name"><i class="folder"></i>\(name)</span><span class="size">—</span></summary>\
            <div class="children">\(inner)</div></details>
            """
    }

    private static func message(for listing: Listing, url: URL) -> String {
        if listing.kind == "unsupported" {
            let inner = escape(url.deletingPathExtension().lastPathComponent)
            return """
                <div class="notice"><i class="file big"></i>
                <p>This file holds a single compressed file, <strong>\(inner)</strong>.</p>
                <p class="dim">Open it with Rcomp to decompress it.</p></div>
                """
        }
        let detail = escape(listing.message ?? "Unknown error.")
        return """
            <div class="notice"><p>Rcomp could not read this archive.</p>
            <p class="dim">\(detail)</p></div>
            """
    }

    static func bytes(_ n: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: n), countStyle: .file)
    }

    static func escape(_ s: String) -> String {
        s.replacingOccurrences(of: "&", with: "&amp;")
            .replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
            .replacingOccurrences(of: "\"", with: "&quot;")
    }

    private static func page(body: String) -> String {
        """
        <!doctype html>
        <html><head><meta charset="utf-8"><style>\(css)</style></head>
        <body>\(body)</body></html>
        """
    }

    // Rows are a fixed 24px tall so the zebra stripes can be a background
    // gradient that stays aligned however deeply folders are nested.
    private static let css = """
        :root { color-scheme: light dark; --fg: #1d1d1f; --dim: #6e6e73; --bg: #ffffff;
          --stripe: #f4f5f5; --line: #d8d8dc; --folder: #4aa3f5; --file: #b9bcc2; }
        @media (prefers-color-scheme: dark) {
          :root { --fg: #f2f2f7; --dim: #98989d; --bg: #1e1e1e; --stripe: #262626;
            --line: #3a3a3c; --folder: #5ab0ff; --file: #8e8e93; }
        }
        * { box-sizing: border-box; }
        html, body { margin: 0; background: var(--bg); color: var(--fg);
          font: 13px -apple-system, BlinkMacSystemFont, sans-serif; }
        header { display: flex; align-items: center; gap: 12px; padding: 12px 16px; }
        header span:first-child { flex: 1; }
        .format { color: var(--dim); text-transform: uppercase; font-size: 11px; letter-spacing: .04em; }
        label { cursor: default; display: flex; align-items: center; gap: 6px; }
        label::before { content: ""; width: 14px; height: 14px; border: 1px solid var(--line);
          border-radius: 4px; background: var(--bg); }
        .toggle { display: none; }
        .toggle:checked ~ header label::before { background: var(--folder); border-color: var(--folder); }
        .columns { display: flex; justify-content: space-between; padding: 4px 16px 4px 34px;
          border-bottom: 1px solid var(--line); font-weight: 600; }
        main { background: repeating-linear-gradient(var(--bg) 0 24px, var(--stripe) 24px 48px);
          min-height: calc(100vh - 80px); }
        .row { display: flex; align-items: center; justify-content: space-between; height: 24px;
          padding: 0 16px 0 34px; list-style: none; white-space: nowrap; }
        .row::-webkit-details-marker { display: none; }
        summary.row { position: relative; cursor: default; }
        summary.row::before { content: "›"; position: absolute; left: 0; width: 30px;
          text-align: right; padding-right: 2px; color: var(--dim); font-size: 15px;
          transition: transform .1s; }
        details[open] > summary.row::before { transform: rotate(90deg); transform-origin: 26px 50%; }
        .children { padding-left: 18px; }
        .name { overflow: hidden; text-overflow: ellipsis; display: flex; align-items: center; gap: 6px; }
        .size { color: var(--dim); padding-left: 16px; }
        i { display: inline-block; flex: none; width: 16px; height: 13px; border-radius: 2px; }
        i.folder { background: var(--folder); clip-path: polygon(0 12%, 40% 12%, 50% 25%, 100% 25%, 100% 100%, 0 100%); }
        i.file { height: 15px; background: var(--file);
          clip-path: polygon(12% 0, 64% 0, 88% 26%, 88% 100%, 12% 100%); }
        i.big { width: 60px; height: 64px; }
        .hidden-item { display: none; }
        .toggle:checked ~ main .hidden-item { display: block; }
        .toggle:checked ~ main .row.hidden-item { display: flex; }
        .notice { padding: 48px 24px; text-align: center; }
        .dim, .more, .empty { color: var(--dim); }
        .more, .empty { padding: 8px 16px; margin: 0; }
        """
}
