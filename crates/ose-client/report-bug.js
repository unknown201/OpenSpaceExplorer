// "Report a bug" in the Game Menu: the server files a replay of the last minutes (the world then and every
// command since) with a note, under replays/<id>.json. A developer runs it with `ose-server --replay` and
// keeps it under crates/ose-server/tests/replays/ as a regression test.
(() => {
  async function report() {
    const note = window.prompt("What went wrong? (a sentence or two; the last few minutes of play are attached)", "");
    if (note === null) return;
    const out = document.getElementById("menu-note");
    const by = (() => { try { return localStorage.getItem("ose.name") || ""; } catch { return ""; } })();
    try {
      const r = await fetch("/api/replays", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ note, by }),
      });
      if (!r.ok) throw new Error(await r.text() || r.statusText);
      const { id } = await r.json();
      if (out) {
        out.textContent = "";
        out.append(`Bug report filed: ${id} `);
        const a = document.createElement("a");
        a.href = `/api/replays/${id}`;
        a.download = `ose-replay-${id}.json`;
        a.textContent = "(download replay)";
        a.dataset.ose = "replay-download";
        out.append(a);
        out.dataset.oseReplay = id;
      }
    } catch (e) {
      if (out) out.textContent = `Could not file the report: ${e.message || e}`;
    }
  }
  document.addEventListener("click", (ev) => {
    if (ev.target instanceof Element && ev.target.closest('[data-ose="report-bug"]')) {
      ev.preventDefault();
      report();
    }
  });
})();
