const http = require("http");

const port = Number(process.env.PORT || 3000);

const server = http.createServer((req, res) => {
  if (req.url === "/healthz") {
    res.writeHead(200, { "content-type": "text/plain" });
    return res.end("ok");
  }
  res.writeHead(200, { "content-type": "text/plain" });
  res.end(`Hello from Ferry (node)! service=${process.env.FERRY_SERVICE_NAME || "?"} deploy=${process.env.FERRY_DEPLOY_ID || "?"}\n`);
});

server.listen(port, "0.0.0.0", () => console.log(`node-hello listening on :${port}`));
process.on("SIGTERM", () => server.close(() => process.exit(0)));
