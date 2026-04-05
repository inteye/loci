import * as vscode from 'vscode';
import * as http from 'http';
import * as https from 'https';
import * as cp from 'child_process';

function serverUrl(): string {
    return vscode.workspace.getConfiguration('sage').get('serverUrl', 'http://localhost:3000');
}

async function post<T>(path: string, body: object): Promise<T> {
    return new Promise((resolve, reject) => {
        const url = new URL(serverUrl() + path);
        const data = JSON.stringify(body);
        const lib = url.protocol === 'https:' ? https : http;
        const req = lib.request({
            hostname: url.hostname, port: url.port || (url.protocol === 'https:' ? 443 : 80),
            path: url.pathname, method: 'POST',
            headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(data) }
        }, res => {
            let buf = '';
            res.on('data', d => buf += d);
            res.on('end', () => {
                try { resolve(JSON.parse(buf)); } catch (e) { reject(e); }
            });
        });
        req.on('error', reject);
        req.write(data);
        req.end();
    });
}

async function checkServer(): Promise<boolean> {
    return new Promise(resolve => {
        const url = new URL(serverUrl() + '/health');
        const lib = url.protocol === 'https:' ? https : http;
        lib.get(url.toString(), res => resolve(res.statusCode === 200))
           .on('error', () => resolve(false));
    });
}

async function ensureServer(context: vscode.ExtensionContext): Promise<boolean> {
    if (await checkServer()) return true;
    const choice = await vscode.window.showWarningMessage(
        'Sage server not running. Start it?',
        'Start sage serve', 'Cancel'
    );
    if (choice !== 'Start sage serve') return false;
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? '.';
    cp.spawn('bs', ['serve', '-p', workspaceRoot], { detached: true, stdio: 'ignore' }).unref();
    // Wait up to 3s for server to start
    for (let i = 0; i < 6; i++) {
        await new Promise(r => setTimeout(r, 500));
        if (await checkServer()) return true;
    }
    vscode.window.showErrorMessage('Could not start sage serve. Run it manually: sage serve');
    return false;
}

// ── commands ──────────────────────────────────────────────────────────────────

async function cmdAsk(context: vscode.ExtensionContext) {
    if (!await ensureServer(context)) return;
    const question = await vscode.window.showInputBox({ prompt: 'Ask about the codebase...' });
    if (!question) return;

    const panel = vscode.window.createWebviewPanel('bs.answer', 'Sage', vscode.ViewColumn.Beside, {});
    panel.webview.html = loadingHtml('Thinking...');

    try {
        const resp = await post<{ answer: string }>('/ask', { question });
        panel.webview.html = markdownHtml(question, resp.answer);
    } catch (e) {
        panel.webview.html = errorHtml(String(e));
    }
}

async function cmdExplain(context: vscode.ExtensionContext) {
    if (!await ensureServer(context)) return;
    const editor = vscode.window.activeTextEditor;
    if (!editor) { vscode.window.showWarningMessage('Open a file first'); return; }

    const filePath = editor.document.uri.fsPath;
    const selection = editor.selection;
    const selectedText = editor.document.getText(selection);

    // If text selected, ask about that; otherwise explain the whole file
    const question = selectedText
        ? `Explain this code:\n\`\`\`\n${selectedText.slice(0, 2000)}\n\`\`\``
        : `Explain the file: ${filePath}`;

    const panel = vscode.window.createWebviewPanel('bs.explain', 'Explain', vscode.ViewColumn.Beside, {});
    panel.webview.html = loadingHtml('Analyzing...');

    try {
        const resp = await post<{ answer: string }>('/ask', { question });
        panel.webview.html = markdownHtml(question, resp.answer);
    } catch (e) {
        panel.webview.html = errorHtml(String(e));
    }
}

async function cmdDiff(context: vscode.ExtensionContext) {
    if (!await ensureServer(context)) return;
    const panel = vscode.window.createWebviewPanel('bs.diff', 'Diff Analysis', vscode.ViewColumn.Beside, {});
    panel.webview.html = loadingHtml('Analyzing changes...');

    try {
        const resp = await post<{ answer: string }>('/ask', {
            question: 'Analyze the recent git changes in this project. What changed and why?'
        });
        panel.webview.html = markdownHtml('Recent Changes', resp.answer);
    } catch (e) {
        panel.webview.html = errorHtml(String(e));
    }
}

async function cmdIndex() {
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!workspaceRoot) { vscode.window.showWarningMessage('No workspace open'); return; }

    vscode.window.withProgress(
        { location: vscode.ProgressLocation.Notification, title: 'Sage: Indexing project...' },
        () => new Promise<void>((resolve, reject) => {
            cp.exec(`sage index "${workspaceRoot}"`, (err, stdout) => {
                if (err) { vscode.window.showErrorMessage(`Index failed: ${err.message}`); reject(err); }
                else { vscode.window.showInformationMessage(`Indexed: ${stdout.trim()}`); resolve(); }
            });
        })
    );
}

// ── html helpers ──────────────────────────────────────────────────────────────

function loadingHtml(msg: string): string {
    return `<!DOCTYPE html><html><body style="font-family:sans-serif;padding:20px">
    <p>${msg}</p></body></html>`;
}

function markdownHtml(title: string, content: string): string {
    const escaped = content.replace(/</g, '&lt;').replace(/>/g, '&gt;');
    return `<!DOCTYPE html><html><body style="font-family:sans-serif;padding:20px;max-width:800px">
    <h3>${title}</h3><pre style="white-space:pre-wrap;background:#f5f5f5;padding:12px;border-radius:4px">${escaped}</pre>
    </body></html>`;
}

function errorHtml(msg: string): string {
    return `<!DOCTYPE html><html><body style="font-family:sans-serif;padding:20px;color:red">
    <p>Error: ${msg}</p><p>Make sure <code>sage serve</code> is running.</p></body></html>`;
}

// ── activate / deactivate ─────────────────────────────────────────────────────

export function activate(context: vscode.ExtensionContext) {
    context.subscriptions.push(
        vscode.commands.registerCommand('bs.ask',     () => cmdAsk(context)),
        vscode.commands.registerCommand('bs.explain', () => cmdExplain(context)),
        vscode.commands.registerCommand('bs.diff',    () => cmdDiff(context)),
        vscode.commands.registerCommand('bs.index',   () => cmdIndex()),
    );
}

export function deactivate() {}
