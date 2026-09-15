ObjC.import('AppKit');
ObjC.import('Foundation');
function run(argv) {
    const destination = ObjC.unwrap($(argv[0]).stringByResolvingSymlinksInPath);
    const running = $.NSWorkspace.sharedWorkspace.runningApplications;
    for (let i = 0; i < running.count; i++) {
        const app = running.objectAtIndex(i);
        if (ObjC.unwrap(app.bundleIdentifier) !== 'run.lns.desktop') continue;
        if (ObjC.unwrap(app.bundleURL.path.stringByResolvingSymlinksInPath) !== destination) continue;
        // JXA invokes zero-argument Objective-C methods on property access.
        const quitAccepted = app.terminate;
        if (!quitAccepted) throw new Error('LNS did not accept the quit request.');
        for (let attempt = 0; attempt < 100 && !app.isTerminated; attempt++) {
            $.NSRunLoop.currentRunLoop.runUntilDate($.NSDate.dateWithTimeIntervalSinceNow(0.1));
        }
        if (!app.isTerminated) throw new Error('LNS did not quit within ten seconds.');
    }
}
