// Quick Look preview extension entry point (data-based preview, macOS 12+).
//
// Declared as NSExtensionPrincipalClass in extension/Info.plist. Quick Look
// grants read access to the previewed file and shows the returned HTML.

import QuickLookUI
import UniformTypeIdentifiers

final class PreviewProvider: QLPreviewProvider, QLPreviewingController {
    func providePreview(for request: QLFilePreviewRequest) async throws -> QLPreviewReply {
        let html = ArchivePreview.render(fileURL: request.fileURL)
        let reply = QLPreviewReply(
            dataOfContentType: .html,
            contentSize: CGSize(width: 820, height: 620)
        ) { _ in Data(html.utf8) }
        reply.stringEncoding = .utf8
        return reply
    }
}
