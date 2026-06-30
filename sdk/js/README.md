# @soma-analytics/client

Zero-dependency browser fetch wrapper for the soma-analytics query API.

```js
import { SomaClient } from "@soma-analytics/client";

const client = new SomaClient(
  () => fetch("/auth/embed-token").then((r) => r.json()).then((d) => d.token),
  { apiUrl: "https://analytics.example.com" }
);

const result = await client.query({
  cube: "events",
  measures: ["events.count"],
  dimensions: ["events.service"],
});

console.log(result.tableData()); // { columns: ["service","count"], rows: [...] }
console.log(result.series());    // [{ name:"events.count", points:[{x:"web",y:42},...] }]
```
