import * as cp from 'child_process';
import * as http from 'http';
import * as https from 'https';
import * as vscode from 'vscode';

function serverUrl(): string {
    const cfg = vscode.workspace.getConfiguration('loci');
    return cfg.get('serverUrl', cfg.get('legacyServerUrl', 'http://localhost:3000'));
}

async function post<T>(path: string, body: object): Promise<T> {
    return new Promise((resolve, reject) => {
        const url = new URL(serverUrl() + path);
        const data = JSON.stringify(body);
        const lib = url.protocol === 'https:' ? https : http;
        const req = lib.request({
            hostname: url.hostname,
            port: url.port || (url.protocol === 'https:' ? 443 : 80),
            path: url.pathname,
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'Content-Length': Buffer.byteLength(data),
            },
        }, res => {
            let buf = '';
            res.on('data', chunk => buf += chunk);
            res.on('end', () => {
                try {
                    resolve(JSON.parse(buf));
                } catch (error) {
                    reject(error);
                }
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

async function ensureServer(): Promise<boolean> {
    if (await checkServer()) return true;

    const choice = await vscode.window.showWarningMessage(
        'loci server not running. Start it?',
        'Start loci serve',
        'Cancel'
    );
    if (choice !== 'Start loci serve') return false;

    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? '.';
    cp.spawn('loci', ['serve', '-p', workspaceRoot], { detached: true, stdio: 'ignore' }).unref();

    for (let i = 0; i < 6; i++) {
        await new Promise(resolve => setTimeout(resolve, 500));
        if (await checkServer()) return true;
    }

    vscode.window.showErrorMessage('Could not start loci serve. Run it manually: loci serve');
    return false;
}

async function cmdAsk() {
    if (!await ensureServer()) return;
    const question = await vscode.window.showInputBox({ prompt: 'Ask about the codebase...' });
    if (!question) return;

    const panel = vscode.window.createWebviewPanel('loci.answer', 'loci Ask', vscode.ViewColumn.Beside, {});
    panel.webview.html = loadingHtml('Thinking...');

    try {
        const resp = await post<{ answer: string }>('/ask', { question });
        panel.webview.html = markdownHtml(question, resp.answer);
    } catch (error) {
        panel.webview.html = errorHtml(String(error));
    }
}

async function cmdExplain() {
    if (!await ensureServer()) return;
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
        vscode.window.showWarningMessage('Open a file first');
        return;
    }

    const filePath = editor.document.uri.fsPath;
    const selectedText = editor.document.getText(editor.selection);
    const panel = vscode.window.createWebviewPanel('loci.explain', 'loci Explain', vscode.ViewColumn.Beside, {});
    panel.webview.html = loadingHtml('Analyzing...');

    try {
        const resp = await post<{ answer: string }>('/explain', {
            target: filePath,
            selected_text: selectedText || null,
        });
        panel.webview.html = markdownHtml(selectedText ? 'Selected Code' : filePath, resp.answer);
    } catch (error) {
        panel.webview.html = errorHtml(String(error));
    }
}

async function cmdDiff() {
    if (!await ensureServer()) return;
    const panel = vscode.window.createWebviewPanel('loci.diff', 'loci Diff', vscode.ViewColumn.Beside, {});
    panel.webview.html = loadingHtml('Analyzing recent changes...');

    try {
        const resp = await post<{ answer: string }>('/diff', {});
        panel.webview.html = markdownHtml('Recent Changes', resp.answer);
    } catch (error) {
        panel.webview.html = errorHtml(String(error));
    }
}

async function cmdIndex() {
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!workspaceRoot) {
        vscode.window.showWarningMessage('No workspace open');
        return;
    }

    vscode.window.withProgress(
        { location: vscode.ProgressLocation.Notification, title: 'loci: Indexing project...' },
        () => new Promise<void>((resolve, reject) => {
            cp.exec(`loci index "${workspaceRoot}"`, (error, stdout) => {
                if (error) {
                    vscode.window.showErrorMessage(`Index failed: ${error.message}`);
                    reject(error);
                    return;
                }
                vscode.window.showInformationMessage(`Indexed: ${stdout.trim()}`);
                resolve();
            });
        })
    );
}

function loadingHtml(message: string): string {
    return `<!DOCTYPE html><html><body style="font-family:sans-serif;padding:20px"><p>${message}</p></body></html>`;
}

function markdownHtml(title: string, content: string): string {
    const escaped = content.replace(/</g, '&lt;').replace(/>/g, '&gt;');
    return `<!DOCTYPE html><html><body style="font-family:sans-serif;padding:20px;max-width:800px">
    <h3>${title}</h3><pre style="white-space:pre-wrap;background:#f5f5f5;padding:12px;border-radius:4px">${escaped}</pre>
    </body></html>`;
}

function errorHtml(message: string): string {
    return `<!DOCTYPE html><html><body style="font-family:sans-serif;padding:20px;color:red">
    <p>Error: ${message}</p><p>Make sure <code>loci serve</code> is running.</p></body></html>`;
}

export function activate(context: vscode.ExtensionContext) {
    context.subscriptions.push(
        vscode.commands.registerCommand('loci.ask', () => cmdAsk()),
        vscode.commands.registerCommand('loci.explain', () => cmdExplain()),
        vscode.commands.registerCommand('loci.diff', () => cmdDiff()),
        vscode.commands.registerCommand('loci.index', () => cmdIndex()),
    );
}

export function deactivate() {}
