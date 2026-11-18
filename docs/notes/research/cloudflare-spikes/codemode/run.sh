B=$1
P(){ printf '%s\n' "--- $1"; curl -s -m 20 -X POST --data-binary "$2" $B/; echo; }
curl -s $B/declarations; echo
P compose 'const a = await tools.query({sql:"select 1"}); const b = await tools.query({sql:"select 2"}); text("n=" + (a[0].rows + b[0].rows)); return {tenant: a[0].tenant, session: b[0].session};'
P fetch 'return await fetch("https://example.com").then(r => r.status);'
P globals 'return {env: typeof env, process: typeof process, caches: typeof caches};'
P unknown 'return await tools.nope({});'
P cpu 'while (true) {}'
P after 'return 1 + 1;'
