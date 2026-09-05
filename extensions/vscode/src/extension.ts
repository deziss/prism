import * as vscode from 'vscode';

function runPrism(args: string[], label: string): void {
    const term = vscode.window.createTerminal({ name: label });
    term.sendText(['prism', ...args].join(' '));
    term.show();
}

export function activate(context: vscode.ExtensionContext) {
    context.subscriptions.push(
        vscode.commands.registerCommand('prism.init', () => {
            runPrism(['init', '--global'], 'PRISM: Init');
        }),
        vscode.commands.registerCommand('prism.gain', () => {
            runPrism(['gain'], 'PRISM: Gain');
        }),
        vscode.commands.registerCommand('prism.memorySearch', async () => {
            const query = await vscode.window.showInputBox({
                prompt: 'Search memory palace',
                placeHolder: 'Enter search query...'
            });
            if (query) {
                runPrism(['memory', 'search', query], 'PRISM: Memory Search');
            }
        }),
        vscode.commands.registerCommand('prism.graphQuery', async () => {
            const query = await vscode.window.showInputBox({
                prompt: 'Query knowledge graph',
                placeHolder: 'Enter query...'
            });
            if (query) {
                runPrism(['graph', 'query', query], 'PRISM: Graph Query');
            }
        }),
        vscode.commands.registerCommand('prism.serve', () => {
            const port = vscode.workspace.getConfiguration('prism').get('prism.proxyPort', 27181);
            const term = vscode.window.createTerminal({ name: 'PRISM: Proxy' });
            term.sendText(`prism serve --port ${port}`);
            term.show();
        })
    );

    vscode.window.showInformationMessage('PRISM — Token Optimizer ready');
}

export function deactivate() {}
