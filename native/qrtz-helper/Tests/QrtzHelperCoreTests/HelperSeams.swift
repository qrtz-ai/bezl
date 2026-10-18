@testable import QrtzHelperCore
@testable import QrtzNano
@testable import QrtzShared

/// Test seams on the monolith wrapper, forwarded to nano's internals (ADR 0146 M2.5: nano's code is its own target).
extension HelperController {
    /// N121: each channel's largest diarizer queue (samples) in the last recording.
    var lastDiarizerPeaks: [CaptureSource: Int] { get async { await nano.lastDiarizerPeaks } }

    func onSamples(_ s: [Float], of owner: NanoController.Recording? = nil, channels: (system: [Float], mic: [Float]?)? = nil) async {
        await nano.onSamples(s, of: owner, channels: channels)
    }

    @discardableResult
    func warmModels() async -> Task<Void, Never>? { await nano.warmModels() }
}
