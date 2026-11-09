import fs from "node:fs";
import http from "node:http";
import path from "node:path";
const root = path.resolve(process.env.STATIC_ROOT || "dist");
const types = {
  ".html": "text/html",
  ".js": "application/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
};
http
  .createServer(async (req, res) => {
    if (!req.url.startsWith("/dashboard")) {
      const upstream = http.request(
        {
          hostname: "127.0.0.1",
          port: Number(process.env.BACKEND_PORT || 19997),
          path: req.url,
          method: req.method,
          headers: req.headers,
        },
        (r) => {
          res.writeHead(r.statusCode, r.headers);
          r.pipe(res);
        }
      );
      upstream.on("error", () => {
        res.writeHead(502);
        res.end("Local backend unavailable");
      });
      req.pipe(upstream);
      return;
    }
    let file;
    try {
      file = path.resolve(
        root,
        "." + decodeURIComponent(req.url.split("?")[0])
      );
    } catch {
      res.writeHead(400);
      res.end("Invalid path");
      return;
    }
    if (fs.existsSync(file) && fs.statSync(file).isDirectory())
      file = path.join(file, "index.html");
    if (!file.startsWith(root + path.sep) || !fs.existsSync(file)) {
      res.writeHead(404);
      res.end("Not found");
      return;
    }
    res.writeHead(200, {
      "content-type": types[path.extname(file)] || "application/octet-stream",
    });
    fs.createReadStream(file).pipe(res);
  })
  .listen(Number(process.env.PORT || 19996), "127.0.0.1");
