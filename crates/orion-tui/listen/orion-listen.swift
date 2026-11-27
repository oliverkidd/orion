// orion-listen: DICTATION's microphone and speech engine, on this Mac.
//
// orion's build compiles this (crates/orion-tui/build.rs) and bakes the
// binary into orion; the TUI writes it out once and runs one per
// dictation (src/dictation.rs). Audio never leaves the Mac.
//
//   orion-listen [--engine auto|analyzer|basic] [--words a,b] [--locale id] [--file x.wav]
//   orion-listen --probe
//
// stdout: one JSON object per line, `ev` naming it —
//   {"ev":"engine","name":"analyzer"|"basic"}
//   {"ev":"download","progress":0.42}      the language's model, first use only
//   {"ev":"listening"}
//   {"ev":"level","v":0.31}                  the mic, 0…1, about 15 a second
//   {"ev":"text","settled":"…","live":"…"}   everything so far: settled, then the live guess
//   {"ev":"done","text":"…"}                 the whole transcript, then exit 0
//   {"ev":"error","kind":"mic-denied"|"speech-denied"|"unsupported"|"failed","message":"…"}
// stdin: a `stop` line, or EOF, finishes: the mic closes, what was heard
// settles, `done` follows. orion kills the process to throw it away.
//
// The engines, best first: SpeechAnalyzer (macOS 26), then the older
// on-device SFSpeechRecognizer (macOS 13+). Neither ever uses a server.

import AVFoundation
import Foundation
import Speech

setvbuf(stdout, nil, _IOLBF, 0)

// MARK: output

let outLock = NSLock()

func emit(_ fields: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: fields),
          let line = String(data: data, encoding: .utf8) else { return }
    outLock.lock()
    print(line)
    fflush(stdout)
    outLock.unlock()
}

func fail(_ kind: String, _ message: String) -> Never {
    emit(["ev": "error", "kind": kind, "message": message])
    exit(1)
}

// MARK: arguments

/// What the command line asked for. Static, so every task reads it.
struct Options: Sendable {
    var engine = "auto"
    var locale = Locale.current
    var words: [String] = []
    var file: String? = nil
    var probe = false

    static let given: Options = {
        var o = Options()
        var args = CommandLine.arguments.dropFirst().makeIterator()
        while let arg = args.next() {
            switch arg {
            case "--engine": o.engine = args.next() ?? o.engine
            case "--locale": o.locale = args.next().map { Locale(identifier: $0) } ?? o.locale
            case "--file": o.file = args.next()
            case "--probe": o.probe = true
            case "--words":
                o.words = (args.next() ?? "").split(separator: ",")
                    .map { $0.trimmingCharacters(in: .whitespaces) }
                    .filter { !$0.isEmpty }
            default: break
            }
        }
        return o
    }()
}

let opts = Options.given
let locale = opts.locale

// MARK: shared state

/// Text the engine has settled, and its live guess after it.
final class Transcript: @unchecked Sendable {
    private let lock = NSLock()
    private var settled = ""
    private var live = ""

    func update(settled newSettled: String? = nil, appendSettled: String? = nil, live newLive: String) {
        lock.lock()
        if let newSettled { settled = newSettled }
        if let appendSettled { settled += appendSettled }
        live = newLive
        let s = settled, l = live
        lock.unlock()
        emit(["ev": "text", "settled": s, "live": l])
    }

    func all() -> String {
        lock.lock()
        defer { lock.unlock() }
        return (settled + live).trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

let transcript = Transcript()

/// The mic's loudness, throttled to about 15 reports a second.
final class Meter: @unchecked Sendable {
    private var last = Date.distantPast
    func feed(_ buffer: AVAudioPCMBuffer) {
        let now = Date()
        guard now.timeIntervalSince(last) >= 0.066, let samples = buffer.floatChannelData?[0] else { return }
        last = now
        let n = Int(buffer.frameLength)
        guard n > 0 else { return }
        var sum: Float = 0
        for i in 0..<n { sum += samples[i] * samples[i] }
        let rms = (sum / Float(n)).squareRoot()
        emit(["ev": "level", "v": Double(min(1, rms * 12))])
    }
}

let meter = Meter()

/// Waits for `stop` (or EOF) on stdin, then runs `onStop` once.
func watchStdin(_ onStop: @escaping @Sendable () -> Void) {
    Thread.detachNewThread {
        while let line = readLine(strippingNewline: true) {
            if line.trimmingCharacters(in: .whitespaces) == "stop" { break }
        }
        onStop()
    }
}

func microphoneAllowed() async -> Bool {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized: return true
    case .notDetermined: return await AVCaptureDevice.requestAccess(for: .audio)
    default: return false
    }
}

func finishProcess() -> Never {
    emit(["ev": "done", "text": transcript.all()])
    exit(0)
}

// MARK: SpeechAnalyzer (macOS 26)

#if compiler(>=6.2)
@available(macOS 26.0, *)
func analyzerLocale() async -> Locale? {
    guard SpeechTranscriber.isAvailable else { return nil }
    return await SpeechTranscriber.supportedLocale(equivalentTo: locale)
}

@available(macOS 26.0, *)
func runAnalyzer(_ loc: Locale) async {
    emit(["ev": "engine", "name": "analyzer"])
    let module = SpeechTranscriber(
        locale: loc,
        transcriptionOptions: [],
        reportingOptions: [.volatileResults, .fastResults],
        attributeOptions: []
    )
    do {
        if let request = try await AssetInventory.assetInstallationRequest(supporting: [module]) {
            emit(["ev": "download", "progress": 0.0])
            let progress = request.progress
            let ticker = Task {
                while !Task.isCancelled {
                    emit(["ev": "download", "progress": progress.fractionCompleted])
                    try? await Task.sleep(nanoseconds: 250_000_000)
                }
            }
            try await request.downloadAndInstall()
            ticker.cancel()
        }
    } catch {
        fail("failed", "couldn't install the speech model: \(error.localizedDescription)")
    }

    let analyzer = SpeechAnalyzer(modules: [module])
    if !Options.given.words.isEmpty {
        let context = AnalysisContext()
        context.contextualStrings[.general] = Options.given.words
        try? await analyzer.setContext(context)
    }
    guard let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [module]) else {
        fail("failed", "no audio format the speech model takes")
    }
    do { try await analyzer.prepareToAnalyze(in: format) } catch {
        fail("failed", "the speech model wouldn't start: \(error.localizedDescription)")
    }

    let results = Task {
        do {
            for try await r in module.results {
                let text = String(r.text.characters)
                if r.isFinal {
                    transcript.update(appendSettled: text, live: "")
                } else {
                    transcript.update(live: text)
                }
            }
        } catch {}
    }

    if let filePath = Options.given.file {
        do {
            let file = try AVAudioFile(forReading: URL(fileURLWithPath: filePath))
            emit(["ev": "listening"])
            try await analyzer.start(inputAudioFile: file, finishAfterFile: true)
            await results.value
        } catch {
            fail("failed", "couldn't read \(filePath): \(error.localizedDescription)")
        }
        finishProcess()
    }

    var feed: AsyncStream<AnalyzerInput>.Continuation!
    let stream = AsyncStream<AnalyzerInput> { feed = $0 }
    let audio = AVAudioEngine()
    let input = audio.inputNode
    let micFormat = input.outputFormat(forBus: 0)
    guard micFormat.sampleRate > 0, let converter = AVAudioConverter(from: micFormat, to: format) else {
        fail("failed", "no microphone to listen to")
    }
    let sink = feed!
    input.installTap(onBus: 0, bufferSize: 2048, format: micFormat) { buffer, _ in
        meter.feed(buffer)
        let ratio = format.sampleRate / micFormat.sampleRate
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 32
        guard let out = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else { return }
        var handed = false
        var error: NSError?
        converter.convert(to: out, error: &error) { _, status in
            if handed {
                status.pointee = .noDataNow
                return nil
            }
            handed = true
            status.pointee = .haveData
            return buffer
        }
        if error == nil, out.frameLength > 0 {
            sink.yield(AnalyzerInput(buffer: out))
        }
    }
    audio.prepare()
    do { try audio.start() } catch {
        fail("failed", "the microphone wouldn't start: \(error.localizedDescription)")
    }
    do { try await analyzer.start(inputSequence: stream) } catch {
        fail("failed", "the speech model wouldn't listen: \(error.localizedDescription)")
    }
    emit(["ev": "listening"])

    watchStdin {
        audio.stop()
        input.removeTap(onBus: 0)
        sink.finish()
        Task {
            try? await analyzer.finalizeAndFinishThroughEndOfInput()
            await results.value
            finishProcess()
        }
    }
}
#endif

// MARK: SFSpeechRecognizer (macOS 13 to 15)

func speechAllowed() async -> Bool {
    switch SFSpeechRecognizer.authorizationStatus() {
    case .authorized: return true
    case .notDetermined:
        return await withCheckedContinuation { done in
            SFSpeechRecognizer.requestAuthorization { done.resume(returning: $0 == .authorized) }
        }
    default: return false
    }
}

/// `semaphore` signalled, or `seconds` gone, without holding up a task.
func waitFor(_ semaphore: DispatchSemaphore, seconds: Double) async {
    await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
        DispatchQueue.global().async {
            _ = semaphore.wait(timeout: .now() + seconds)
            done.resume()
        }
    }
}

func runBasic() async {
    emit(["ev": "engine", "name": "basic"])
    guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable else {
        fail("unsupported", "this Mac has no speech recognizer for \(locale.identifier)")
    }
    guard recognizer.supportsOnDeviceRecognition else {
        fail("unsupported", "this Mac can't recognise \(locale.identifier) on-device")
    }
    guard await speechAllowed() else {
        fail("speech-denied", "Speech Recognition is off for your terminal in System Settings")
    }
    let request = SFSpeechAudioBufferRecognitionRequest()
    request.requiresOnDeviceRecognition = true
    request.shouldReportPartialResults = true
    if #available(macOS 13.0, *) { request.addsPunctuation = true }
    if !Options.given.words.isEmpty { request.contextualStrings = Options.given.words }

    let finished = DispatchSemaphore(value: 0)
    let task = recognizer.recognitionTask(with: request) { result, error in
        if let result {
            let text = result.bestTranscription.formattedString
            if result.isFinal {
                transcript.update(settled: text, live: "")
                finished.signal()
            } else {
                transcript.update(live: text)
            }
        } else if error != nil {
            finished.signal()
        }
    }

    if let filePath = Options.given.file {
        guard let file = try? AVAudioFile(forReading: URL(fileURLWithPath: filePath)),
              let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length)),
              (try? file.read(into: buffer)) != nil else {
            fail("failed", "couldn't read \(filePath)")
        }
        emit(["ev": "listening"])
        request.append(buffer)
        request.endAudio()
        await waitFor(finished, seconds: 30)
        finishProcess()
    }

    let audio = AVAudioEngine()
    let input = audio.inputNode
    let micFormat = input.outputFormat(forBus: 0)
    guard micFormat.sampleRate > 0 else { fail("failed", "no microphone to listen to") }
    input.installTap(onBus: 0, bufferSize: 2048, format: micFormat) { buffer, _ in
        meter.feed(buffer)
        request.append(buffer)
    }
    audio.prepare()
    do { try audio.start() } catch {
        fail("failed", "the microphone wouldn't start: \(error.localizedDescription)")
    }
    emit(["ev": "listening"])

    watchStdin {
        audio.stop()
        input.removeTap(onBus: 0)
        request.endAudio()
        // The final result settles everything; a recognizer that never
        // sends one leaves what it last guessed.
        _ = finished.wait(timeout: .now() + 3)
        task.cancel()
        finishProcess()
    }
}

// MARK: main

func analyzerReady() async -> Locale? {
    #if compiler(>=6.2)
    if #available(macOS 26.0, *) { return await analyzerLocale() }
    #endif
    return nil
}

if opts.probe {
    let analyzer = await analyzerReady()
    emit([
        "ev": "probe",
        "analyzer": analyzer?.identifier ?? "",
        "basic": SFSpeechRecognizer(locale: locale)?.supportsOnDeviceRecognition ?? false,
        "microphone": AVCaptureDevice.authorizationStatus(for: .audio).rawValue,
        "speech": SFSpeechRecognizer.authorizationStatus().rawValue,
    ])
    exit(0)
}

if opts.file == nil {
    guard await microphoneAllowed() else {
        fail("mic-denied", "the microphone is off for your terminal in System Settings")
    }
}

var started = false
#if compiler(>=6.2)
if opts.engine != "basic", #available(macOS 26.0, *), let loc = await analyzerLocale() {
    started = true
    await runAnalyzer(loc)
}
#endif
if !started {
    if opts.engine == "analyzer" { fail("unsupported", "SpeechAnalyzer needs macOS 26") }
    await runBasic()
}

while true { try? await Task.sleep(nanoseconds: 3_600_000_000_000) }
