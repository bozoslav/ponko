// The browser's own output (its process's stdout and stderr, launch and
// exit), kept for the run's artifacts. A browser that hangs or crashes
// often says why only there: a failed audio or GPU process, a missing
// library. Playwright prints it through its `pw:browser` debug channel;
// this module turns that channel on and keeps its lines here instead of
// the console. Import it before playwright-core, which reads DEBUG once.

const CHANNEL = "pw:browser";
const KEEP = 2000;

/** The lines the browser printed, newest last (at most KEEP). */
export const browserLog = [];

const asked = (process.env.DEBUG ?? "").split(",").includes(CHANNEL);
if (!asked) {
  process.env.DEBUG = [process.env.DEBUG, CHANNEL].filter(Boolean).join(",");
  // Plain lines (a timestamp, the channel, the text), also on a terminal.
  process.env.DEBUG_COLORS = "no";
  const write = process.stderr.write.bind(process.stderr);
  process.stderr.write = (chunk, ...rest) => {
    const text = typeof chunk === "string" ? chunk : Buffer.from(chunk).toString("utf8");
    if (!text.includes(` ${CHANNEL} `)) return write(chunk, ...rest);
    for (const line of text.split("\n")) {
      if (line.trim()) browserLog.push(line.replace(/^\S+ pw:browser /, ""));
    }
    if (browserLog.length > KEEP) browserLog.splice(0, browserLog.length - KEEP);
    const done = rest.find((arg) => typeof arg === "function");
    if (done) done();
    return true;
  };
}
