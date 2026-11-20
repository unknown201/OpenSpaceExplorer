OpenSpaceExplorer for macOS (Apple Silicon and Intel)

1. Unzip, then open Terminal in this folder (or type `cd ` and drag the folder onto Terminal).
2. Run:   bash start.command
   The first run downloads the game assets (about 3.8 GB, once) from ose.jfly.dev into ./assets,
   then starts the server and opens http://localhost:7777/ in your browser.
   After that, double-clicking start.command also works. Ctrl+C in Terminal stops the server.

Sign-in: this local server has no email, so leave the email box empty and sign in with just a
pilot name. Your browser remembers the session.

Your accounts and saves live in ~/Library/Application Support/OpenSpaceExplorer, outside this
folder, so a newer package can replace this one without losing anything.

Use Safari 26+, or Chrome/Edge (WebGPU). Other port: OSE_PORT=8080 bash start.command
Already have the PC's exported assets\web folder on this Mac? ./fetch-assets.sh /path/to/web
copies it instead of downloading.
