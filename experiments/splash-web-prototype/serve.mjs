import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { extname } from 'node:path';

const types = new Map([
  ['.html', 'text/html; charset=utf-8'], ['.css', 'text/css; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'], ['.png', 'image/png'],
]);
const root = new URL('.', import.meta.url);
const port = Number(process.env.PORT ?? 4173);

createServer(async (req, res) => {
  try {
    const path = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
    const file = path === '/' ? 'index.html' : path.slice(1);
    const url = new URL(encodeURI(file), root);
    if (!url.href.startsWith(root.href) || !types.has(extname(file))) {
      res.writeHead(404).end();
      return;
    }
    const body = await readFile(url);
    res.writeHead(200, { 'Content-Type': types.get(extname(file)) }).end(body);
  } catch {
    res.writeHead(404).end();
  }
}).listen(port, '127.0.0.1', () => {
  console.log(`Agentelier Splash · http://127.0.0.1:${port}`);
});
