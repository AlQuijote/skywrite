// Premiere Pro import check for Skywrite's Premiere XML (ExtendScript).
//
// UNTESTED: written on Linux without Premiere. Run it on a Mac or PC with
// Premiere Pro open on a scratch project, for example from VS Code with the
// Adobe "ExtendScript Debugger" extension (target: Premiere Pro), and record
// the result with `skywrite nit`.
//
// It imports XML_PATH, finds the sequence the import created, counts clips per
// video and audio track, checks that no imported media is offline, and prints
// one line starting with PASS or FAIL (also returned as the script's result).
//
// Edit the four settings below. Expected counts are per Premiere track (a
// stereo pair in the XML is ONE Premiere audio track). Example: two camera
// clips on V1, a title on V2, the camera's sound kept on one of them, a
// recorder track and a music bed give video [2, 1] and audio [1, 1, 1].

var XML_PATH = "/Users/editor/Projects/sintel/edits/cut.mac.xml";
var EXPECT_VIDEO = [2, 1];
var EXPECT_AUDIO = [1, 1, 1];
var EXPECT_FPS = 23.976; // sequence frame rate, compared to 0.01

function trackCounts(tracks) {
    var counts = [];
    for (var i = 0; i < tracks.numTracks; i++) {
        counts.push(tracks[i].clips.numItems);
    }
    // Premiere may add empty tracks; ignore trailing empty ones.
    while (counts.length > 0 && counts[counts.length - 1] === 0) {
        counts.pop();
    }
    return counts;
}

function sameList(a, b) {
    if (a.length !== b.length) {
        return false;
    }
    for (var i = 0; i < a.length; i++) {
        if (a[i] !== b[i]) {
            return false;
        }
    }
    return true;
}

function offlineClips(tracks) {
    var names = [];
    for (var i = 0; i < tracks.numTracks; i++) {
        var clips = tracks[i].clips;
        for (var j = 0; j < clips.numItems; j++) {
            var item = clips[j].projectItem;
            if (item && item.isOffline()) {
                names.push(clips[j].name);
            }
        }
    }
    return names;
}

function run() {
    var file = new File(XML_PATH);
    if (!file.exists) {
        return "FAIL: " + XML_PATH + " does not exist";
    }
    var before = {};
    for (var i = 0; i < app.project.sequences.numSequences; i++) {
        before[app.project.sequences[i].sequenceID] = true;
    }
    var ok = app.project.importFiles([file.fsName], true, app.project.getInsertionBin(), false);
    if (!ok) {
        return "FAIL: importFiles returned false";
    }
    var sequence = null;
    for (var k = 0; k < app.project.sequences.numSequences; k++) {
        var candidate = app.project.sequences[k];
        if (!before[candidate.sequenceID]) {
            sequence = candidate;
        }
    }
    if (sequence === null) {
        return "FAIL: the import created no sequence";
    }
    var video = trackCounts(sequence.videoTracks);
    var audio = trackCounts(sequence.audioTracks);
    var offline = offlineClips(sequence.videoTracks).concat(offlineClips(sequence.audioTracks));
    var fps = 1 / sequence.getSettings().videoFrameRate.seconds;
    var problems = [];
    if (!sameList(video, EXPECT_VIDEO)) {
        problems.push("video clips per track " + video.join(",") + " (expected " + EXPECT_VIDEO.join(",") + ")");
    }
    if (!sameList(audio, EXPECT_AUDIO)) {
        problems.push("audio clips per track " + audio.join(",") + " (expected " + EXPECT_AUDIO.join(",") + ")");
    }
    if (Math.abs(fps - EXPECT_FPS) > 0.01) {
        problems.push("sequence fps " + fps + " (expected " + EXPECT_FPS + ")");
    }
    if (offline.length > 0) {
        problems.push("offline media: " + offline.join(", "));
    }
    var summary = "sequence '" + sequence.name + "' video " + video.join(",") + " audio " + audio.join(",") + " fps " + fps.toFixed(3);
    return problems.length === 0 ? "PASS: " + summary : "FAIL: " + problems.join("; ") + " | " + summary;
}

var result;
try {
    result = run();
} catch (error) {
    result = "FAIL: script error " + error;
}
$.writeln(result);
result;
