function Nu(n){let e=n.replace(/^#\/?/,"").split("/").filter(Boolean).map(decodeURIComponent);return{screen:e[0]??"overview",params:e.slice(1)}}function vn(n,...e){return`#/${[n,...e.map(t=>encodeURIComponent(String(t)))].join("/")}`}function _n(n,...e){location.hash=vn(n,...e)}function Uu(n){window.addEventListener("hashchange",()=>n(Nu(location.hash))),n(Nu(location.hash))}var bs=class extends Error{constructor(t,i){super(i);this.status=t}status};async function Ms(n,e,t){let i=await fetch(e,{method:n,headers:t===void 0?{}:{"content-type":"application/json"},body:t===void 0?void 0:JSON.stringify(t)}),r=await i.text(),s=r?JSON.parse(r):null;if(!i.ok){let o=s?.error??`${n} ${e}: ${i.status}`;throw new bs(i.status,o)}return s}var Gt=n=>Ms("GET",n),Gn=(n,e={})=>Ms("POST",n,e),Bl=(n,e)=>Ms("PUT",n,e),Fu=(n,e)=>Ms("PATCH",n,e),yo=n=>Ms("DELETE",n),Oe={overview:()=>Gt("/v1/overview"),providerKinds:()=>Gt("/v1/provider-kinds"),providers:()=>Gt("/v1/providers"),addProvider:n=>Gn("/v1/providers",n),updateProvider:n=>Bl(`/v1/providers/${n.id}`,n),removeProvider:n=>yo(`/v1/providers/${n}`),setSecret:(n,e)=>Bl(`/v1/providers/${n}/secret`,{value:e}),testProvider:(n,e)=>Gn(`/v1/providers/${n}/test`,e?{model:e}:{}),providerModels:n=>Gt(`/v1/providers/${n}/models`),models:()=>Gt("/v1/models"),settings:()=>Gt("/v1/settings"),saveSettings:n=>Bl("/v1/settings",n),agents:()=>Gt("/v1/agents"),agent:n=>Gt(`/v1/agents/${n}`),spawnAgents:(n,e,t)=>Gn("/v1/agents",{role:n,count:e,model:t}),setAgentModel:(n,e)=>Fu(`/v1/agents/${n}`,{model:e}),renameAgent:(n,e)=>Fu(`/v1/agents/${n}`,{name:e}),killAgent:n=>yo(`/v1/agents/${n}`),chat:(n,e)=>Gn(`/v1/agents/${n}/chat`,{text:e}),assign:(n,e)=>Gn(`/v1/agents/${n}/assign`,{task_id:e}),cancel:n=>Gn(`/v1/agents/${n}/cancel`),stop:n=>Gn(`/v1/agents/${n}/stop`),transcript:(n,e=0,t=1e3)=>Gt(`/v1/agents/${n}/transcript?after=${e}&limit=${t}`),tasks:()=>Gt("/v1/tasks").then(n=>n.tasks),task:n=>Gt(`/v1/tasks/${n}`),createTask:n=>Gn("/v1/tasks",n),withdrawTask:n=>yo(`/v1/tasks/${n}`),board:()=>Gt("/v1/board").then(n=>n.posts),conversations:()=>Gt("/v1/conversations").then(n=>n.conversations),motions:()=>Gt("/v1/motions").then(n=>n.motions),decide:(n,e,t)=>Gn(`/v1/motions/${n}/decide`,{decision:e,reason:t}),treasury:()=>Gt("/v1/treasury"),treasuryJournal:()=>Gt("/v1/treasury/journal").then(n=>Array.isArray(n)?n:n.entries),treasuryWallets:()=>Gt("/v1/treasury/wallets"),mintBudget:n=>Gn("/v1/treasury/mint",{amount:n}),fineAgent:(n,e,t)=>Gn("/v1/treasury/fine",{cid:n,amount:e,memo:t}),agora:()=>Gt("/v1/agora").then(n=>n.calls),machine:()=>Gt("/v1/machine"),machines:()=>Gt("/v1/machines"),runs:()=>Gt("/v1/runs").then(n=>n.runs),run:n=>Gt(`/v1/runs/${n}`),createRun:(n,e)=>Gn("/v1/runs",{n,vcpus:1,memory_mib:256,role:e}),killRun:n=>yo(`/v1/runs/${n}`)};var Hl=new Map,Ss=null,Vl=!1,Gl=new Set;function bt(n,e){let t=Hl.get(n);return t||Hl.set(n,t=new Set),t.add(e),()=>t.delete(e)}function zu(n){return Gl.add(n),n(Vl),()=>Gl.delete(n)}var qf=["agent","work","transcript","delta","task","treasury","agora","board","conversation","motion","provider","run","vm","serial"];function $f(){if(!Ss){Ss=new EventSource("/v1/events"),Ss.onopen=()=>Ou(!0),Ss.onerror=()=>Ou(!1);for(let n of qf)Ss.addEventListener(n,e=>{let t;try{t=JSON.parse(e.data)}catch{return}Hl.get(n)?.forEach(i=>i(t))})}}function Ou(n){n!==Vl&&(Vl=n,Gl.forEach(e=>e(n)),n&&nr())}var bo=class{constructor(e){this.v=e}v;fns=new Set;get(){return this.v}set(e){this.v=e,this.fns.forEach(t=>t(e))}watch(e){return this.fns.add(e),e(this.v),()=>this.fns.delete(e)}},ut=new bo([]),ir=new bo(null),zl=null;function nr(){return zl??=Oe.agents().then(n=>ut.set(n.sort((e,t)=>e.cid-t.cid))).catch(()=>{}).finally(()=>zl=null),zl}function Bu(){return Oe.overview().then(n=>ir.set(n)).catch(()=>ir.set(null))}function Yf(n){let e=ut.get().slice(),t=e.findIndex(i=>i.cid===n.cid);return t>=0?e[t]={...e[t],...n}:e.push(n),e.sort((i,r)=>i.cid-r.cid)}function Hu(){$f(),nr(),Bu(),bt("agent",n=>{ut.set(Yf(n)),nr()}),bt("work",()=>{nr()}),setInterval(()=>{Bu()},1e4),setInterval(()=>{nr()},15e3)}var Vu={anthropic:{title:"Anthropic",hex:"191919",path:"M17.3041 3.541h-3.6718l6.696 16.918H24Zm-10.6082 0L0 20.459h3.7442l1.3693-3.5527h7.0052l1.3693 3.5528h3.7442L10.5363 3.5409Zm-.3712 10.2232 2.2914-5.9456 2.2914 5.9456Z"},openai:{title:"OpenAI",hex:"412991",path:"M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z"},google:{title:"Google Gemini",hex:"8E75B2",path:"M11.04 19.32Q12 21.51 12 24q0-2.49.93-4.68.96-2.19 2.58-3.81t3.81-2.55Q21.51 12 24 12q-2.49 0-4.68-.93a12.3 12.3 0 0 1-3.81-2.58 12.3 12.3 0 0 1-2.58-3.81Q12 2.49 12 0q0 2.49-.96 4.68-.93 2.19-2.55 3.81a12.3 12.3 0 0 1-3.81 2.58Q2.49 12 0 12q2.49 0 4.68.96 2.19.93 3.81 2.55t2.55 3.81"},aws:{title:"Amazon AWS",hex:"232F3E",path:"M6.763 10.036c0 .296.032.535.088.71.064.176.144.368.256.576.04.063.056.127.056.183 0 .08-.048.16-.152.24l-.503.335a.383.383 0 0 1-.208.072c-.08 0-.16-.04-.239-.112a2.47 2.47 0 0 1-.287-.375 6.18 6.18 0 0 1-.248-.471c-.622.734-1.405 1.101-2.347 1.101-.67 0-1.205-.191-1.596-.574-.391-.384-.59-.894-.59-1.533 0-.678.239-1.23.726-1.644.487-.415 1.133-.623 1.955-.623.272 0 .551.024.846.064.296.04.6.104.918.176v-.583c0-.607-.127-1.03-.375-1.277-.255-.248-.686-.367-1.3-.367-.28 0-.568.031-.863.103-.295.072-.583.16-.862.272a2.287 2.287 0 0 1-.28.104.488.488 0 0 1-.127.023c-.112 0-.168-.08-.168-.247v-.391c0-.128.016-.224.056-.28a.597.597 0 0 1 .224-.167c.279-.144.614-.264 1.005-.36a4.84 4.84 0 0 1 1.246-.151c.95 0 1.644.216 2.091.647.439.43.662 1.085.662 1.963v2.586zm-3.24 1.214c.263 0 .534-.048.822-.144.287-.096.543-.271.758-.51.128-.152.224-.32.272-.512.047-.191.08-.423.08-.694v-.335a6.66 6.66 0 0 0-.735-.136 6.02 6.02 0 0 0-.75-.048c-.535 0-.926.104-1.19.32-.263.215-.39.518-.39.917 0 .375.095.655.295.846.191.2.47.296.838.296zm6.41.862c-.144 0-.24-.024-.304-.08-.064-.048-.12-.16-.168-.311L7.586 5.55a1.398 1.398 0 0 1-.072-.32c0-.128.064-.2.191-.2h.783c.151 0 .255.025.31.08.065.048.113.16.16.312l1.342 5.284 1.245-5.284c.04-.16.088-.264.151-.312a.549.549 0 0 1 .32-.08h.638c.152 0 .256.025.32.08.063.048.12.16.151.312l1.261 5.348 1.381-5.348c.048-.16.104-.264.16-.312a.52.52 0 0 1 .311-.08h.743c.127 0 .2.065.2.2 0 .04-.009.08-.017.128a1.137 1.137 0 0 1-.056.2l-1.923 6.17c-.048.16-.104.263-.168.311a.51.51 0 0 1-.303.08h-.687c-.151 0-.255-.024-.32-.08-.063-.056-.119-.16-.15-.32l-1.238-5.148-1.23 5.14c-.04.16-.087.264-.15.32-.065.056-.177.08-.32.08zm10.256.215c-.415 0-.83-.048-1.229-.143-.399-.096-.71-.2-.918-.32-.128-.071-.215-.151-.247-.223a.563.563 0 0 1-.048-.224v-.407c0-.167.064-.247.183-.247.048 0 .096.008.144.024.048.016.12.048.2.08.271.12.566.215.878.279.319.064.63.096.95.096.502 0 .894-.088 1.165-.264a.86.86 0 0 0 .415-.758.777.777 0 0 0-.215-.559c-.144-.151-.416-.287-.807-.415l-1.157-.36c-.583-.183-1.014-.454-1.277-.813a1.902 1.902 0 0 1-.4-1.158c0-.335.073-.63.216-.886.144-.255.335-.479.575-.654.24-.184.51-.32.83-.415.32-.096.655-.136 1.006-.136.175 0 .359.008.535.032.183.024.35.056.518.088.16.04.312.08.455.127.144.048.256.096.336.144a.69.69 0 0 1 .24.2.43.43 0 0 1 .071.263v.375c0 .168-.064.256-.184.256a.83.83 0 0 1-.303-.096 3.652 3.652 0 0 0-1.532-.311c-.455 0-.815.071-1.062.223-.248.152-.375.383-.375.71 0 .224.08.416.24.567.159.152.454.304.877.44l1.134.358c.574.184.99.44 1.237.767.247.327.367.702.367 1.117 0 .343-.072.655-.207.926-.144.272-.336.511-.583.703-.248.2-.543.343-.886.447-.36.111-.734.167-1.142.167zM21.698 16.207c-2.626 1.94-6.442 2.969-9.722 2.969-4.598 0-8.74-1.7-11.87-4.526-.247-.223-.024-.527.272-.351 3.384 1.963 7.559 3.153 11.877 3.153 2.914 0 6.114-.607 9.06-1.852.439-.2.814.287.383.607zM22.792 14.961c-.336-.43-2.22-.207-3.074-.103-.255.032-.295-.192-.063-.36 1.5-1.053 3.967-.75 4.254-.399.287.36-.08 2.826-1.485 4.007-.215.184-.423.088-.327-.151.32-.79 1.03-2.57.695-2.994z"},azure:{title:"Microsoft Azure",hex:"0078D4",path:"M22.379 23.343a1.62 1.62 0 0 0 1.536-2.14v.002L17.35 1.76A1.62 1.62 0 0 0 15.816.657H8.184A1.62 1.62 0 0 0 6.65 1.76L.086 21.204a1.62 1.62 0 0 0 1.536 2.139h4.741a1.62 1.62 0 0 0 1.535-1.103l.977-2.892 4.947 3.675c.28.208.618.32.966.32m-3.084-12.531 3.624 10.739a.54.54 0 0 1-.51.713v-.001h-.03a.54.54 0 0 1-.322-.106l-9.287-6.9h4.853m6.313 7.006c.116-.326.13-.694.007-1.058L9.79 1.76a1.722 1.722 0 0 0-.007-.02h6.034a.54.54 0 0 1 .512.366l6.562 19.445a.54.54 0 0 1-.338.684"},xai:{title:"X",hex:"000000",path:"M14.234 10.162 22.977 0h-2.072l-7.591 8.824L7.251 0H.258l9.168 13.343L.258 24H2.33l8.016-9.318L16.749 24h6.993zm-2.837 3.299-.929-1.329L3.076 1.56h3.182l5.965 8.532.929 1.329 7.754 11.09h-3.182z"},deepseek:{title:"DeepSeek",hex:"5786FE",path:"M23.748 4.651c-.254-.124-.364.113-.512.233-.051.04-.094.09-.137.137-.372.397-.806.657-1.373.626-.829-.046-1.537.214-2.163.848-.133-.782-.575-1.248-1.247-1.548-.352-.155-.708-.311-.955-.65-.172-.24-.219-.509-.305-.774-.055-.16-.11-.323-.293-.35-.2-.031-.278.136-.356.276-.313.572-.434 1.202-.422 1.84.027 1.436.633 2.58 1.838 3.393.137.094.172.187.129.323-.082.28-.18.553-.266.833-.055.179-.137.218-.328.14a5.5 5.5 0 0 1-1.737-1.179c-.857-.828-1.631-1.743-2.597-2.46a12 12 0 0 0-.689-.47c-.985-.957.13-1.743.387-1.836.27-.098.094-.433-.778-.428-.872.003-1.67.295-2.687.685a3 3 0 0 1-.465.136 9.6 9.6 0 0 0-2.883-.101c-1.885.21-3.39 1.1-4.497 2.622C.082 8.776-.231 10.854.152 13.02c.403 2.284 1.568 4.175 3.36 5.653 1.857 1.533 3.997 2.284 6.438 2.14 1.482-.085 3.132-.284 4.994-1.86.47.234.962.328 1.78.398.629.058 1.235-.031 1.705-.129.735-.155.684-.836.418-.961-2.155-1.004-1.682-.595-2.112-.926 1.095-1.295 2.768-3.598 3.284-6.733.05-.346.115-.834.108-1.114-.004-.171.035-.238.23-.257a4.2 4.2 0 0 0 1.545-.475c1.397-.763 1.96-2.016 2.093-3.517.02-.23-.004-.467-.247-.588M11.58 18.168c-2.088-1.642-3.101-2.183-3.52-2.16-.39.024-.32.472-.234.763.09.288.207.487.371.74.114.167.192.416-.113.603-.673.416-1.842-.14-1.897-.168-1.361-.801-2.5-1.86-3.301-3.306-.775-1.393-1.225-2.888-1.299-4.482-.02-.385.094-.522.477-.592a4.7 4.7 0 0 1 1.53-.038c2.131.311 3.946 1.264 5.467 2.774.868.86 1.525 1.887 2.202 2.89.72 1.066 1.494 2.082 2.48 2.915.348.291.626.513.892.677-.802.09-2.14.109-3.055-.615zm1.001-6.44a.306.306 0 0 1 .415-.287.3.3 0 0 1 .113.074.3.3 0 0 1 .086.214c0 .17-.136.307-.308.307a.303.303 0 0 1-.306-.307m3.11 1.596c-.2.081-.4.151-.591.16a1.25 1.25 0 0 1-.798-.254c-.274-.23-.47-.358-.551-.758a1.7 1.7 0 0 1 .015-.588c.07-.327-.007-.537-.238-.727-.188-.156-.426-.199-.689-.199a.6.6 0 0 1-.254-.078.253.253 0 0 1-.114-.358 1 1 0 0 1 .192-.21c.356-.202.767-.136 1.146.016.352.144.618.408 1.001.782.392.451.462.576.685.915.176.264.336.536.446.848.066.194-.02.353-.25.45"},moonshot:{title:"Moonshot AI",hex:"000000",path:"m1.053 16.91 9.538 2.55a21 20.981 0 0 0 .06 2.031l5.956 1.592a12 11.99 0 0 1-15.554-6.172m-1.02-5.79 11.352 3.035a21 20.981 0 0 0-.469 2.01l10.817 2.89a12 11.99 0 0 1-1.845 2.004L.658 15.918a12 11.99 0 0 1-.625-4.796m1.593-5.146L13.573 9.17a21 20.981 0 0 0-1.01 1.874l11.297 3.02a21 20.981 0 0 1-.67 2.362l-11.55-3.087L.125 10.26a12 11.99 0 0 1 1.499-4.285ZM6.067 1.58l11.285 3.016a21 20.981 0 0 0-1.688 1.719l7.824 2.091a21 20.981 0 0 1 .513 2.664L2.107 5.218a12 11.99 0 0 1 3.96-3.638M21.68 4.866 7.222 1.003A12 11.99 0 0 1 21.68 4.866"},mistral:{title:"Mistral AI",hex:"FA520F",path:"M17.143 3.429v3.428h-3.429v3.429h-3.428V6.857H6.857V3.43H3.43v13.714H0v3.428h10.286v-3.428H6.857v-3.429h3.429v3.429h3.429v-3.429h3.428v3.429h-3.428v3.428H24v-3.428h-3.43V3.429z"},openrouter:{title:"OpenRouter",hex:"94A3B8",path:"M16.778 1.844v1.919q-.569-.026-1.138-.032-.708-.008-1.415.037c-1.93.126-4.023.728-6.149 2.237-2.911 2.066-2.731 1.95-4.14 2.75-.396.223-1.342.574-2.185.798-.841.225-1.753.333-1.751.333v4.229s.768.108 1.61.333c.842.224 1.789.575 2.185.799 1.41.798 1.228.683 4.14 2.75 2.126 1.509 4.22 2.11 6.148 2.236.88.058 1.716.041 2.555.005v1.918l7.222-4.168-7.222-4.17v2.176c-.86.038-1.611.065-2.278.021-1.364-.09-2.417-.357-3.979-1.465-2.244-1.593-2.866-2.027-3.68-2.508.889-.518 1.449-.906 3.822-2.59 1.56-1.109 2.614-1.377 3.978-1.466.667-.044 1.418-.017 2.278.02v2.176L24 6.014Z"},perplexity:{title:"Perplexity",hex:"1FB8CD",path:"M22.3977 7.0896h-2.3106V.0676l-7.5094 6.3542V.1577h-1.1554v6.1966L4.4904 0v7.0896H1.6023v10.3976h2.8882V24l6.932-6.3591v6.2005h1.1554v-6.0469l6.9318 6.1807v-6.4879h2.8882V7.0896zm-3.4657-4.531v4.531h-5.355l5.355-4.531zm-13.2862.0676 4.8691 4.4634H5.6458V2.6262zM2.7576 16.332V8.245h7.8476l-6.1149 6.1147v1.9723H2.7576zm2.8882 5.0404v-3.8852h.0001v-2.6488l5.7763-5.7764v7.0111l-5.7764 5.2993zm12.7086.0248-5.7766-5.1509V9.0618l5.7766 5.7766v6.5588zm2.8882-5.0652h-1.733v-1.9723L13.3948 8.245h7.8478v8.087z"},nvidia:{title:"NVIDIA",hex:"76B900",path:"M8.948 8.798v-1.43a6.7 6.7 0 0 1 .424-.018c3.922-.124 6.493 3.374 6.493 3.374s-2.774 3.851-5.75 3.851c-.398 0-.787-.062-1.158-.185v-4.346c1.528.185 1.837.857 2.747 2.385l2.04-1.714s-1.492-1.952-4-1.952a6.016 6.016 0 0 0-.796.035m0-4.735v2.138l.424-.027c5.45-.185 9.01 4.47 9.01 4.47s-4.08 4.964-8.33 4.964c-.37 0-.733-.035-1.095-.097v1.325c.3.035.61.062.91.062 3.957 0 6.82-2.023 9.593-4.408.459.371 2.34 1.263 2.73 1.652-2.633 2.208-8.772 3.984-12.253 3.984-.335 0-.653-.018-.971-.053v1.864H24V4.063zm0 10.326v1.131c-3.657-.654-4.673-4.46-4.673-4.46s1.758-1.944 4.673-2.262v1.237H8.94c-1.528-.186-2.73 1.245-2.73 1.245s.68 2.412 2.739 3.11M2.456 10.9s2.164-3.197 6.5-3.533V6.201C4.153 6.59 0 10.653 0 10.653s2.35 6.802 8.948 7.42v-1.237c-4.84-.6-6.492-5.936-6.492-5.936z"},ollama:{title:"Ollama",hex:"000000",path:"M16.361 10.26a.894.894 0 0 0-.558.47l-.072.148.001.207c0 .193.004.217.059.353.076.193.152.312.291.448.24.238.51.3.872.205a.86.86 0 0 0 .517-.436.752.752 0 0 0 .08-.498c-.064-.453-.33-.782-.724-.897a1.06 1.06 0 0 0-.466 0zm-9.203.005c-.305.096-.533.32-.65.639a1.187 1.187 0 0 0-.06.52c.057.309.31.59.598.667.362.095.632.033.872-.205.14-.136.215-.255.291-.448.055-.136.059-.16.059-.353l.001-.207-.072-.148a.894.894 0 0 0-.565-.472 1.02 1.02 0 0 0-.474.007Zm4.184 2c-.131.071-.223.25-.195.383.031.143.157.288.353.407.105.063.112.072.117.136.004.038-.01.146-.029.243-.02.094-.036.194-.036.222.002.074.07.195.143.253.064.052.076.054.255.059.164.005.198.001.264-.03.169-.082.212-.234.15-.525-.052-.243-.042-.28.087-.355.137-.08.281-.219.324-.314a.365.365 0 0 0-.175-.48.394.394 0 0 0-.181-.033c-.126 0-.207.03-.355.124l-.085.053-.053-.032c-.219-.13-.259-.145-.391-.143a.396.396 0 0 0-.193.032zm.39-2.195c-.373.036-.475.05-.654.086-.291.06-.68.195-.951.328-.94.46-1.589 1.226-1.787 2.114-.04.176-.045.234-.045.53 0 .294.005.357.043.524.264 1.16 1.332 2.017 2.714 2.173.3.033 1.596.033 1.896 0 1.11-.125 2.064-.727 2.493-1.571.114-.226.169-.372.22-.602.039-.167.044-.23.044-.523 0-.297-.005-.355-.045-.531-.288-1.29-1.539-2.304-3.072-2.497a6.873 6.873 0 0 0-.855-.031zm.645.937a3.283 3.283 0 0 1 1.44.514c.223.148.537.458.671.662.166.251.26.508.303.82.02.143.01.251-.043.482-.08.345-.332.705-.672.957a3.115 3.115 0 0 1-.689.348c-.382.122-.632.144-1.525.138-.582-.006-.686-.01-.853-.042-.57-.107-1.022-.334-1.35-.68-.264-.28-.385-.535-.45-.946-.03-.192.025-.509.137-.776.136-.326.488-.73.836-.963.403-.269.934-.46 1.422-.512.187-.02.586-.02.773-.002zm-5.503-11a1.653 1.653 0 0 0-.683.298C5.617.74 5.173 1.666 4.985 2.819c-.07.436-.119 1.04-.119 1.503 0 .544.064 1.24.155 1.721.02.107.031.202.023.208a8.12 8.12 0 0 1-.187.152 5.324 5.324 0 0 0-.949 1.02 5.49 5.49 0 0 0-.94 2.339 6.625 6.625 0 0 0-.023 1.357c.091.78.325 1.438.727 2.04l.13.195-.037.064c-.269.452-.498 1.105-.605 1.732-.084.496-.095.629-.095 1.294 0 .67.009.803.088 1.266.095.555.288 1.143.503 1.534.071.128.243.393.264.407.007.003-.014.067-.046.141a7.405 7.405 0 0 0-.548 1.873c-.062.417-.071.552-.071.991 0 .56.031.832.148 1.279L3.42 24h1.478l-.05-.091c-.297-.552-.325-1.575-.068-2.597.117-.472.25-.819.498-1.296l.148-.29v-.177c0-.165-.003-.184-.057-.293a.915.915 0 0 0-.194-.25 1.74 1.74 0 0 1-.385-.543c-.424-.92-.506-2.286-.208-3.451.124-.486.329-.918.544-1.154a.787.787 0 0 0 .223-.531c0-.195-.07-.355-.224-.522a3.136 3.136 0 0 1-.817-1.729c-.14-.96.114-2.005.69-2.834.563-.814 1.353-1.336 2.237-1.475.199-.033.57-.028.776.01.226.04.367.028.512-.041.179-.085.268-.19.374-.431.093-.215.165-.333.36-.576.234-.29.46-.489.822-.729.413-.27.884-.467 1.352-.561.17-.035.25-.04.569-.04.319 0 .398.005.569.04a4.07 4.07 0 0 1 1.914.997c.117.109.398.457.488.602.034.057.095.177.132.267.105.241.195.346.374.43.14.068.286.082.503.045.343-.058.607-.053.943.016 1.144.23 2.14 1.173 2.581 2.437.385 1.108.276 2.267-.296 3.153-.097.15-.193.27-.333.419-.301.322-.301.722-.001 1.053.493.539.801 1.866.708 3.036-.062.772-.26 1.463-.533 1.854a2.096 2.096 0 0 1-.224.258.916.916 0 0 0-.194.25c-.054.109-.057.128-.057.293v.178l.148.29c.248.476.38.823.498 1.295.253 1.008.231 2.01-.059 2.581a.845.845 0 0 0-.044.098c0 .006.329.009.732.009h.73l.02-.074.036-.134c.019-.076.057-.3.088-.516.029-.217.029-1.016 0-1.258-.11-.875-.295-1.57-.597-2.226-.032-.074-.053-.138-.046-.141.008-.005.057-.074.108-.152.376-.569.607-1.284.724-2.228.031-.26.031-1.378 0-1.628-.083-.645-.182-1.082-.348-1.525a6.083 6.083 0 0 0-.329-.7l-.038-.064.131-.194c.402-.604.636-1.262.727-2.04a6.625 6.625 0 0 0-.024-1.358 5.512 5.512 0 0 0-.939-2.339 5.325 5.325 0 0 0-.95-1.02 8.097 8.097 0 0 1-.186-.152.692.692 0 0 1 .023-.208c.208-1.087.201-2.443-.017-3.503-.19-.924-.535-1.658-.98-2.082-.354-.338-.716-.482-1.15-.455-.996.059-1.8 1.205-2.116 3.01a6.805 6.805 0 0 0-.097.726c0 .036-.007.066-.015.066a.96.96 0 0 1-.149-.078A4.857 4.857 0 0 0 12 3.03c-.832 0-1.687.243-2.456.698a.958.958 0 0 1-.148.078c-.008 0-.015-.03-.015-.066a6.71 6.71 0 0 0-.097-.725C8.997 1.392 8.337.319 7.46.048a2.096 2.096 0 0 0-.585-.041Zm.293 1.402c.248.197.523.759.682 1.388.03.113.06.244.069.292.007.047.026.152.041.233.067.365.098.76.102 1.24l.002.475-.12.175-.118.178h-.278c-.324 0-.646.041-.954.124l-.238.06c-.033.007-.038-.003-.057-.144a8.438 8.438 0 0 1 .016-2.323c.124-.788.413-1.501.696-1.711.067-.05.079-.049.157.013zm9.825-.012c.17.126.358.46.498.888.28.854.36 2.028.212 3.145-.019.14-.024.151-.057.144l-.238-.06a3.693 3.693 0 0 0-.954-.124h-.278l-.119-.178-.119-.175.002-.474c.004-.669.066-1.19.214-1.772.157-.623.434-1.185.68-1.382.078-.062.09-.063.159-.012z"},lmstudio:{title:"LM Studio",hex:"000000",path:"M14.025 0c3.492 0 5.237 0 6.571.68a6.24 6.24 0 0 1 2.725 2.724C24 4.738 24 6.484 24 9.975v4.05c0 3.492 0 5.237-.68 6.571a6.24 6.24 0 0 1-2.724 2.725c-1.334.679-3.08.679-6.571.679h-4.05c-3.492 0-5.237 0-6.571-.68A6.24 6.24 0 0 1 .68 20.597C0 19.262 0 17.516 0 14.025v-4.05c0-3.492 0-5.237.68-6.571A6.23 6.23 0 0 1 3.404.68C4.738 0 6.484 0 9.975 0zM7.688 16.313a1.313 1.313 0 0 0 0 2.625h11.625a1.313 1.313 0 0 0 0-2.625zm-3-3.75a1.313 1.313 0 0 0 0 2.624h11.625a1.313 1.313 0 0 0 0-2.624zm3-3.75a1.313 1.313 0 0 0 0 2.624h11.625a1.313 1.313 0 0 0 0-2.624zm-3-3.75a1.313 1.313 0 0 0 0 2.625h11.625a1.313 1.313 0 0 0 0-2.625z"},vllm:{title:"vLLM",hex:"30A2FF",path:"m23.6 0-8.721 4.59L9.829 24h7.41zM9.83 24V5.142H.4Z"},huggingface:{title:"Hugging Face",hex:"FFD21E",path:"M12.025 1.13c-5.77 0-10.449 4.647-10.449 10.378 0 1.112.178 2.181.503 3.185.064-.222.203-.444.416-.577a.96.96 0 0 1 .524-.15c.293 0 .584.124.84.284.278.173.48.408.71.694.226.282.458.611.684.951v-.014c.017-.324.106-.622.264-.874s.403-.487.762-.543c.3-.047.596.06.787.203s.31.313.4.467c.15.257.212.468.233.542.01.026.653 1.552 1.657 2.54.616.605 1.01 1.223 1.082 1.912.055.537-.096 1.059-.38 1.572.637.121 1.294.187 1.967.187.657 0 1.298-.063 1.921-.178-.287-.517-.44-1.041-.384-1.581.07-.69.465-1.307 1.081-1.913 1.004-.987 1.647-2.513 1.657-2.539.021-.074.083-.285.233-.542.09-.154.208-.323.4-.467a1.08 1.08 0 0 1 .787-.203c.359.056.604.29.762.543s.247.55.265.874v.015c.225-.34.457-.67.683-.952.23-.286.432-.52.71-.694.257-.16.547-.284.84-.285a.97.97 0 0 1 .524.151c.228.143.373.388.43.625l.006.04a10.3 10.3 0 0 0 .534-3.273c0-5.731-4.678-10.378-10.449-10.378M8.327 6.583a1.5 1.5 0 0 1 .713.174 1.487 1.487 0 0 1 .617 2.013c-.183.343-.762-.214-1.102-.094-.38.134-.532.914-.917.71a1.487 1.487 0 0 1 .69-2.803m7.486 0a1.487 1.487 0 0 1 .689 2.803c-.385.204-.536-.576-.916-.71-.34-.12-.92.437-1.103.094a1.487 1.487 0 0 1 .617-2.013 1.5 1.5 0 0 1 .713-.174m-10.68 1.55a.96.96 0 1 1 0 1.921.96.96 0 0 1 0-1.92m13.838 0a.96.96 0 1 1 0 1.92.96.96 0 0 1 0-1.92M8.489 11.458c.588.01 1.965 1.157 3.572 1.164 1.607-.007 2.984-1.155 3.572-1.164.196-.003.305.12.305.454 0 .886-.424 2.328-1.563 3.202-.22-.756-1.396-1.366-1.63-1.32q-.011.001-.02.006l-.044.026-.01.008-.03.024q-.018.017-.035.036l-.032.04a1 1 0 0 0-.058.09l-.014.025q-.049.088-.11.19a1 1 0 0 1-.083.116 1.2 1.2 0 0 1-.173.18q-.035.029-.075.058a1.3 1.3 0 0 1-.251-.243 1 1 0 0 1-.076-.107c-.124-.193-.177-.363-.337-.444-.034-.016-.104-.008-.2.022q-.094.03-.216.087-.06.028-.125.063l-.13.074q-.067.04-.136.086a3 3 0 0 0-.135.096 3 3 0 0 0-.26.219 2 2 0 0 0-.12.121 2 2 0 0 0-.106.128l-.002.002a2 2 0 0 0-.09.132l-.001.001a1.2 1.2 0 0 0-.105.212q-.013.036-.024.073c-1.139-.875-1.563-2.317-1.563-3.203 0-.334.109-.457.305-.454m.836 10.354c.824-1.19.766-2.082-.365-3.194-1.13-1.112-1.789-2.738-1.789-2.738s-.246-.945-.806-.858-.97 1.499.202 2.362c1.173.864-.233 1.45-.685.64-.45-.812-1.683-2.896-2.322-3.295s-1.089-.175-.938.647 2.822 2.813 2.562 3.244-1.176-.506-1.176-.506-2.866-2.567-3.49-1.898.473 1.23 2.037 2.16c1.564.932 1.686 1.178 1.464 1.53s-3.675-2.511-4-1.297c-.323 1.214 3.524 1.567 3.287 2.405-.238.839-2.71-1.587-3.216-.642-.506.946 3.49 2.056 3.522 2.064 1.29.33 4.568 1.028 5.713-.624m5.349 0c-.824-1.19-.766-2.082.365-3.194 1.13-1.112 1.789-2.738 1.789-2.738s.246-.945.806-.858.97 1.499-.202 2.362c-1.173.864.233 1.45.685.64.451-.812 1.683-2.896 2.322-3.295s1.089-.175.938.647-2.822 2.813-2.562 3.244 1.176-.506 1.176-.506 2.866-2.567 3.49-1.898-.473 1.23-2.037 2.16c-1.564.932-1.686 1.178-1.464 1.53s3.675-2.511 4-1.297c.323 1.214-3.524 1.567-3.287 2.405.238.839 2.71-1.587 3.216-.642.506.946-3.49 2.056-3.522 2.064-1.29.33-4.568 1.028-5.713-.624"}};var Gu={overview:[["rect",{width:"7",height:"9",x:"3",y:"3",rx:"1"}],["rect",{width:"7",height:"5",x:"14",y:"3",rx:"1"}],["rect",{width:"7",height:"9",x:"14",y:"12",rx:"1"}],["rect",{width:"7",height:"5",x:"3",y:"16",rx:"1"}]],agents:[["path",{d:"M12 8V4H8"}],["rect",{width:"16",height:"12",x:"4",y:"8",rx:"2"}],["path",{d:"M2 14h2"}],["path",{d:"M20 14h2"}],["path",{d:"M15 13v2"}],["path",{d:"M9 13v2"}]],tasks:[["path",{d:"M13 5h8"}],["path",{d:"M13 12h8"}],["path",{d:"M13 19h8"}],["path",{d:"m3 17 2 2 4-4"}],["path",{d:"m3 7 2 2 4-4"}]],models:[["path",{d:"M12 20v2"}],["path",{d:"M12 2v2"}],["path",{d:"M17 20v2"}],["path",{d:"M17 2v2"}],["path",{d:"M2 12h2"}],["path",{d:"M2 17h2"}],["path",{d:"M2 7h2"}],["path",{d:"M20 12h2"}],["path",{d:"M20 17h2"}],["path",{d:"M20 7h2"}],["path",{d:"M7 20v2"}],["path",{d:"M7 2v2"}],["rect",{x:"4",y:"4",width:"16",height:"16",rx:"2"}],["rect",{x:"8",y:"8",width:"8",height:"8",rx:"1"}]],conversations:[["path",{d:"M16 10a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 14.286V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"}],["path",{d:"M20 9a2 2 0 0 1 2 2v10.286a.71.71 0 0 1-1.212.502l-2.202-2.202A2 2 0 0 0 17.172 19H10a2 2 0 0 1-2-2v-1"}]],board:[["path",{d:"M11 6a13 13 0 0 0 8.4-2.8A1 1 0 0 1 21 4v12a1 1 0 0 1-1.6.8A13 13 0 0 0 11 14H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2z"}],["path",{d:"M6 14a12 12 0 0 0 2.4 7.2 2 2 0 0 0 3.2-2.4A8 8 0 0 1 10 14"}],["path",{d:"M8 6v8"}]],council:[["path",{d:"M10 18v-7"}],["path",{d:"M11.119 2.205a2 2 0 0 1 1.762 0l7.84 3.846A.5.5 0 0 1 20.5 7h-17a.5.5 0 0 1-.22-.949z"}],["path",{d:"M14 18v-7"}],["path",{d:"M18 18v-7"}],["path",{d:"M3 22h18"}],["path",{d:"M6 18v-7"}]],treasury:[["path",{d:"M13.744 17.736a6 6 0 1 1-7.48-7.48"}],["path",{d:"M15 6h1v4"}],["path",{d:"m6.134 14.768.866-.5 2 3.464"}],["circle",{cx:"16",cy:"8",r:"6"}]],floor:[["path",{d:"M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"}],["path",{d:"m3.3 7 8.7 5 8.7-5"}],["path",{d:"M12 22V12"}]],machine:[["rect",{width:"20",height:"8",x:"2",y:"2",rx:"2",ry:"2"}],["rect",{width:"20",height:"8",x:"2",y:"14",rx:"2",ry:"2"}],["line",{x1:"6",x2:"6.01",y1:"6",y2:"6"}],["line",{x1:"6",x2:"6.01",y1:"18",y2:"18"}]],plus:[["path",{d:"M5 12h14"}],["path",{d:"M12 5v14"}]],send:[["path",{d:"M3.714 3.048a.498.498 0 0 0-.683.627l2.843 7.627a2 2 0 0 1 0 1.396l-2.842 7.627a.498.498 0 0 0 .682.627l18-8.5a.5.5 0 0 0 0-.904z"}],["path",{d:"M6 12h16"}]],stop:[["rect",{width:"18",height:"18",x:"3",y:"3",rx:"2"}]],x:[["path",{d:"M18 6 6 18"}],["path",{d:"m6 6 12 12"}]],check:[["path",{d:"M20 6 9 17l-5-5"}]],alert:[["path",{d:"m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"}],["path",{d:"M12 9v4"}],["path",{d:"M12 17h.01"}]],play:[["path",{d:"M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"}]],refresh:[["path",{d:"M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"}],["path",{d:"M21 3v5h-5"}],["path",{d:"M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"}],["path",{d:"M8 16H3v5"}]],key:[["path",{d:"M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z"}],["circle",{cx:"16.5",cy:"7.5",r:".5",fill:"currentColor"}]],trash:[["path",{d:"M10 11v6"}],["path",{d:"M14 11v6"}],["path",{d:"M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"}],["path",{d:"M3 6h18"}],["path",{d:"M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"}]],edit:[["path",{d:"M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"}],["path",{d:"m15 5 4 4"}]],chevronRight:[["path",{d:"m9 18 6-6-6-6"}]],chevronDown:[["path",{d:"m6 9 6 6 6-6"}]],terminal:[["path",{d:"M12 19h8"}],["path",{d:"m4 17 6-6-6-6"}]],wrench:[["path",{d:"M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.106-3.105c.32-.322.863-.22.983.218a6 6 0 0 1-8.259 7.057l-7.91 7.91a1 1 0 0 1-2.999-3l7.91-7.91a6 6 0 0 1 7.057-8.259c.438.12.54.662.219.984z"}]],brain:[["path",{d:"M12 18V5"}],["path",{d:"M15 13a4.17 4.17 0 0 1-3-4 4.17 4.17 0 0 1-3 4"}],["path",{d:"M17.598 6.5A3 3 0 1 0 12 5a3 3 0 1 0-5.598 1.5"}],["path",{d:"M17.997 5.125a4 4 0 0 1 2.526 5.77"}],["path",{d:"M18 18a4 4 0 0 0 2-7.464"}],["path",{d:"M19.967 17.483A4 4 0 1 1 12 18a4 4 0 1 1-7.967-.517"}],["path",{d:"M6 18a4 4 0 0 1-2-7.464"}],["path",{d:"M6.003 5.125a4 4 0 0 0-2.526 5.77"}]],user:[["path",{d:"M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"}],["circle",{cx:"12",cy:"7",r:"4"}]],orchestrator:[["rect",{x:"16",y:"16",width:"6",height:"6",rx:"1"}],["rect",{x:"2",y:"16",width:"6",height:"6",rx:"1"}],["rect",{x:"9",y:"2",width:"6",height:"6",rx:"1"}],["path",{d:"M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3"}],["path",{d:"M12 12V8"}]],search:[["path",{d:"m21 21-4.34-4.34"}],["circle",{cx:"11",cy:"11",r:"8"}]],external:[["path",{d:"M15 3h6v6"}],["path",{d:"M10 14 21 3"}],["path",{d:"M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"}]],clock:[["circle",{cx:"12",cy:"12",r:"10"}],["path",{d:"M12 6v6l4 2"}]],zap:[["path",{d:"M15.914 4a1.5 1.5 0 00-2.474-1.561l-9 9A1.5 1.5 0 005.5 14h4.002a.5.5 0 01.471.666L8.086 20a1.5 1.5 0 002.475 1.56l9-9A1.5 1.5 0 0018.5 10h-3.997a.5.5 0 01-.472-.667z"}]],file:[["path",{d:"M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"}],["path",{d:"M14 2v5a1 1 0 0 0 1 1h5"}],["path",{d:"M10 9H8"}],["path",{d:"M16 13H8"}],["path",{d:"M16 17H8"}]],settings:[["path",{d:"M10 5H3"}],["path",{d:"M12 19H3"}],["path",{d:"M14 3v4"}],["path",{d:"M16 17v4"}],["path",{d:"M21 12h-9"}],["path",{d:"M21 19h-5"}],["path",{d:"M21 5h-7"}],["path",{d:"M8 10v4"}],["path",{d:"M8 12H3"}]],ban:[["circle",{cx:"12",cy:"12",r:"10"}],["path",{d:"M4.929 4.929 19.07 19.071"}]],copy:[["rect",{width:"14",height:"14",x:"8",y:"8",rx:"2",ry:"2"}],["path",{d:"M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"}]],sparkles:[["path",{d:"M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z"}],["path",{d:"M20 2v4"}],["path",{d:"M22 4h-4"}],["circle",{cx:"4",cy:"20",r:"2"}]]};function v(n,e,...t){let[i,...r]=n.split("."),s=document.createElement(i);if(r.length&&(s.className=r.join(" ")),e)for(let[o,a]of Object.entries(e))a==null||a===!1||(o.startsWith("on")&&typeof a=="function"?s.addEventListener(o.slice(2),a):o==="class"?s.className=[s.className,String(a)].filter(Boolean).join(" "):a===!0?s.setAttribute(o,""):s.setAttribute(o,String(a)));return Wu(s,t),s}function Wu(n,e){for(let t of e.flat())t==null||t===!1||n.appendChild(typeof t=="object"?t:document.createTextNode(String(t)))}function Ue(n,...e){for(;n.firstChild;)n.removeChild(n.firstChild);Wu(n,e)}var Mo="http://www.w3.org/2000/svg";function li(n,e=""){let t=document.createElementNS(Mo,"svg");t.setAttribute("viewBox","0 0 24 24"),t.setAttribute("class",`icon ${e}`.trim()),t.setAttribute("aria-hidden","true");for(let[i,r]of Gu[n]){let s=document.createElementNS(Mo,i);for(let[o,a]of Object.entries(r))s.setAttribute(o,a);t.appendChild(s)}return t}function Xu(n,e="0 0 24 24"){let t=document.createElementNS(Mo,"svg");t.setAttribute("viewBox",e),t.setAttribute("aria-hidden","true");let i=document.createElementNS(Mo,"path");return i.setAttribute("d",n),t.appendChild(i),t}function ot(n,e="muted",t=!1){return v("span.badge",{class:e==="muted"?"":e},t?v("span.dot",{class:e}):null,n)}function ci(n){return v("span.dot",{class:n})}function We(n,e,t={}){let i=[t.kind,t.size,n?"":"icon-only"].filter(Boolean).join(" "),r=v("button.btn",{class:i,type:"button",title:t.title??null,disabled:!!t.disabled},t.icon?li(t.icon):null,n||null);return r.addEventListener("click",e),r}function zt(n,e,t={}){return v("section.card",null,n!=null?v("header.card-head",null,typeof n=="string"?v("h2",null,n):n,...t.actions??[]):null,v("div.card-body",{class:t.flush?"flush":""},e))}function Zn(n,e,t){return v("div.card.stat",null,v("span.label",null,n),v("span.value",null,String(e)),t?v("span.sub",null,t):null)}function ht(n,e,t,i){return v("div.empty",null,li(n),v("h3",null,e),t?v("p",null,t):null,i??null)}function rr(n,e,t){return v("table.table",null,v("thead",null,v("tr",null,n.map(i=>v("th",{class:i.num?"num":"",style:i.width?`width:${i.width}`:null},i.label)))),v("tbody",null,e.map(i=>{let r=v("tr",{class:t?"clickable":""},n.map(s=>v("td",{class:s.num?"num":""},s.cell(i))));return t&&r.addEventListener("click",()=>t(i)),r})))}function gt(n,e,t){return v("label.field",null,v("span",null,n),e,t?v("span.hint",null,t):null)}function Zt(n={}){return v("input.input",n)}function Kn(n={}){return v("textarea.textarea",n)}function fn(n,e){let t=v("select.select",null,n.map(i=>v("option",{value:i.value},i.label)));return e!==void 0&&(t.value=e),t}function qu(n,e,t){return v("div.tabs",{role:"tablist"},n.map(i=>{let r=v("button",{role:"tab","aria-selected":i.key===e?"true":"false"},i.label);return r.addEventListener("click",()=>t(i.key)),r}))}function Cn(n,e,t){let i=()=>{s.remove(),document.removeEventListener("keydown",r),t?.()},r=o=>{o.key==="Escape"&&i()},s=v("div.modal-backdrop",null,v("div.modal",{role:"dialog","aria-modal":"true","aria-label":n},v("header.card-head",null,v("h2",null,n),We("",i,{kind:"ghost",icon:"x",title:"Close"})),v("div.card-body",null,e)));return s.addEventListener("mousedown",o=>{o.target===s&&i()}),document.addEventListener("keydown",r),document.body.appendChild(s),s.querySelector("input, textarea, select, button.primary")?.focus(),i}function nn(n,e="info"){let t=document.querySelector(".toast-host");t||(t=v("div.toast-host",{"aria-live":"polite"}),document.body.appendChild(t));let i=v("div.toast",{class:e==="error"?"error":""},n);t.appendChild(i),setTimeout(()=>i.remove(),e==="error"?6e3:3e3)}async function dt(n,e){try{let t=await n();return e&&nn(e),t}catch(t){nn(t instanceof Error?t.message:String(t),"error");return}}var Zf={zai:"Z",groq:"Gq",together:"Tg",fireworks:"Fw",cerebras:"Cb",llamacpp:"ll",mock:"M"};function Wl(n,e=!1){let t=v("span.logo",{class:e?"lg":"",title:n}),i=Vu[n];if(i){t.appendChild(Xu(i.path));let r=i.hex.toLowerCase(),s=parseInt(r.slice(0,2),16)+parseInt(r.slice(2,4),16)+parseInt(r.slice(4,6),16)<150;t.style.color=s?"var(--text)":`#${r}`}else Ue(t,v("span.mono-mark",null,Zf[n]??n.slice(0,2).toUpperCase()));return t}function Wt(n,e=Date.now()){if(!n)return"\u2014";let t=Math.max(0,Math.round((e-n)/1e3));return t<5?"just now":t<60?`${t}s ago`:t<3600?`${Math.floor(t/60)}m ago`:t<86400?`${Math.floor(t/3600)}h ago`:`${Math.floor(t/86400)}d ago`}function ws(n){return n<1e3?String(n):n<1e6?`${(n/1e3).toFixed(n<1e4?1:0)}k`:`${(n/1e6).toFixed(1)}M`}function Kt(n,e){return n.length>e?`${n.slice(0,e-1)}\u2026`:n}var Kf={orchestrator:"Orchestrator",architect:"Architect",auditor:"Auditor",breeder:"Breeder",builder:"Builder","edge-operator":"Edge operator",finance:"Finance",incident:"Incident","legal-agents-counsel":"Legal \xB7 agents' counsel","legal-agents-defender":"Legal \xB7 agents' defender","legal-agents-ombudsman":"Legal \xB7 agents' ombudsman","legal-gov-judge":"Legal \xB7 government judge","legal-gov-prosecutor":"Legal \xB7 government prosecutor",librarian:"Librarian",mayor:"Mayor","mayor-office":"Mayor \xB7 office",planner:"Planner",qa:"QA",release:"Release",researcher:"Researcher",reviewer:"Reviewer","security-reviewer":"Security reviewer"};function yn(n){return Kf[n]??n}function pn(n){return n.name.trim()===""?"Unnamed":n.name}var Jf=80;function $u(n,e){let t=v("header.agent-head"),i=v("div.transcript"),r=Kn({placeholder:"Message this agent\u2026",rows:3}),s=We("Send",()=>{k()},{kind:"primary",icon:"send",title:"Send (Ctrl+Enter)"}),o=v("select.select"),a=v("div.model-line"),c=v("div"),l=v("div.stack"),h=new Map,m=new Map,u=[],g,_=!0,x=!1,f=!1,d=0,S,P=F=>{let H=i.scrollHeight-i.scrollTop-i.clientHeight<Jf;F(),H&&(i.scrollTop=i.scrollHeight)},w=F=>{let H=h.get(F);if(H)return H;let q=v("header.work-head"),ee=v("section.work",null,q),J={root:ee,head:q};return h.set(F,J),i.append(ee),J},T=F=>{for(let[H,q]of m)q.work===F&&(q.el.remove(),m.delete(H))},A=F=>{if(F.seq<=d||(d=F.seq,F.kind==="model_request"))return;let H=F.data;P(()=>{let q=w(F.work);switch(F.kind){case"work_start":Ue(q.head,ot(String(H.kind??"work"),"info"),v("p.work-prompt",null,String(H.prompt??"")));break;case"work_end":{let ee=String(H.status??""),J=/fail|error/.test(ee)?"danger":/cancel|stop/.test(ee)?"warn":"ok";q.root.append(v("div.work-end",null,ot(ee||"done",J)));break}case"model_response":T(F.work),q.root.append(...jf(H));break;case"tool_decision":{let ee=Qf(H);ee&&q.root.append(v("div.decision",null,ee));break}case"tool_result":{let ee=H.is_error===!0,J=String(H.content??H.output??"");q.root.append(v("details.result",null,v("summary",{class:ee?"danger":"ok"},ee?"Error":"Output"),v("pre.pre",null,J)));break}case"error":q.root.append(v("p.danger",null,String(H.message??"Error")));break;default:break}})},D=F=>{let H=F;if(H.cid!==e)return;let q=`${H.work}:${H.req}`;P(()=>{let ee=m.get(q);if(!ee){let ne=new Text,ce=new Text,pe=v("div.live-turn",null,v("p.prose",null,ne));ee={work:H.work,el:pe,text:ne,thinking:ce},m.set(q,ee),w(H.work).root.append(pe)}let J=H.delta;J.type==="tool_use"?ee.el.append(v("span.chip",null,`\u2192 ${J.name}`)):J.type==="text"&&ee.text.appendData(J.text)})},p=async()=>{let F=await dt(()=>Oe.transcript(e));if(_){for(let H of(F??[]).slice().sort((q,ee)=>q.seq-ee.seq))A(H);x=!0;for(let H of u.splice(0))H();i.scrollTop=i.scrollHeight}},M=F=>{let H=S??"",q=F.map(ee=>({value:`${ee.provider}/${ee.id}`,label:ee.name??ee.id}));H&&!q.some(ee=>ee.value===H)&&q.push({value:H,label:H}),o.replaceChildren(v("option",{value:""},"Use the default"),...q.map(ee=>v("option",{value:ee.value},ee.label))),o.value=H},y=()=>{let F=g;if(!F)return;let H=F.status==="offline",q=F.status==="working"?"Working":F.status==="idle"?"Idle":"Offline",ee=F.status==="working"?"info":F.status==="idle"?"ok":"muted";Ue(t,v("a.back",{href:vn("agents")},"Agents"),v("div.agent-title",null,v("h1",null,pn(F)),ot(q,ee,!0),ot(F.tier==="orchestrator"?"Orchestrator":"Worker",F.tier==="orchestrator"?"accent":"muted"),v("span.muted",null,yn(F.role))),v("div.row.agent-actions",null,F.status==="working"?We("Cancel work",()=>{dt(()=>Oe.cancel(e),"Cancelled")},{icon:"ban"}):null,We("Stop",()=>{dt(()=>Oe.stop(e),"Stopped")},{kind:"danger",icon:"stop"}))),r.disabled=H,s.disabled=H||f;let J=F.model_source==="agent"?"set for this agent":F.model_source==="role"?"from role":"default";Ue(a,v("strong",null,F.model),v("span.faint",null,J));let ne=F.model_override??"";ne!==S&&(S=ne,o.value=ne),Ue(c,v("dl.kv",null,v("dt",null,"CID"),v("dd.mono",null,String(F.cid)),v("dt",null,"Run"),v("dd.mono",null,F.run??"\u2014"),v("dt",null,"VM"),v("dd.mono",null,F.vm===null?"\u2014":String(F.vm)),v("dt",null,"Connected"),v("dd",null,Wt(F.connected_at_ms)),v("dt",null,"Last seen"),v("dd",null,Wt(F.last_seen_ms)),v("dt",null,"Tokens in"),v("dd.mono",null,ws(F.usage.input_tokens)),v("dt",null,"Tokens out"),v("dd.mono",null,ws(F.usage.output_tokens)),v("dt",null,"Credits"),v("dd.mono",null,String(F.credits)))),Ue(l,F.work?v("div.row",null,ot(F.work.kind,"info"),v("span.truncate",null,Kt(F.work.prompt,160))):v("span.faint",null,"Nothing running"),v("span.muted",null,`${F.queue} queued`))},C=()=>g?.status==="offline",k=async()=>{let F=r.value.trim();if(!F||f||C())return;f=!0,s.disabled=!0;let H=await dt(()=>Oe.chat(e,F));f=!1,s.disabled=C(),H&&(r.value="")};r.addEventListener("keydown",F=>{F.key==="Enter"&&(F.metaKey||F.ctrlKey)&&(F.preventDefault(),k())}),o.addEventListener("change",()=>{let F=o.value;dt(()=>Oe.setAgentModel(e,F||null),"Model updated").then(H=>{H?S=F:o.value=S??""})});let L=[ut.watch(F=>{let H=F.find(q=>q.cid===e);H&&(g=H,y())}),bt("transcript",F=>{let H=F;H.cid===e&&(x?A(H):u.push(()=>A(H)))}),bt("delta",F=>{F.cid===e&&(x?D(F):u.push(()=>D(F)))})];return n.append(v("div.agent-view",null,t,v("div.agent-grid",null,v("div.agent-main",null,zt(null,i,{flush:!0}),v("div.composer",null,r,v("div.row",null,v("span",{style:"flex:1"}),s))),v("aside.agent-side",null,zt("Model",v("div.stack",null,a,gt("Change model",o))),zt("Details",c),zt("Queue",l))))),(async()=>{try{g=await Oe.agent(e)}catch(F){if(!_)return;Ue(n,F instanceof bs&&F.status===404?ht("agents","No such agent",void 0,v("a.btn",{href:vn("agents")},"All agents")):ht("alert","Could not load this agent",F instanceof Error?F.message:void 0));return}_&&(y(),Oe.models().then(F=>_&&M(F.models)).catch(()=>{}),await p())})(),()=>{_=!1,L.forEach(F=>F()),m.clear()}}function jf(n){let e=Array.isArray(n.content)?n.content:[],t=[];for(let i of e)i.type==="text"?t.push(v("p.prose",null,i.text)):i.type==="tool_use"&&t.push(v("details.tool",null,v("summary.chip",null,`\u2192 ${i.name}`),v("pre.pre",null,JSON.stringify(i.input,null,2))));return t}function Qf(n){switch(n.decision){case"allow":return ot("ran in VM","info");case"done":return ot("ran on host","muted");case"deny":return ot(`denied: ${String(n.reason??"")}`,"danger");default:return null}}var ep={working:"info",idle:"ok",offline:"muted"},tp={task:"accent",chat:"info",message:"muted",motion:"warn"},Xl=16;function Yu(n){let e="",t="all",i=[],r=fn([{value:"",label:"All roles"}]),s=fn([{value:"all",label:"All statuses"},{value:"working",label:"Working"},{value:"idle",label:"Idle"},{value:"offline",label:"Offline"}]),o=v("div"),a=We("Spawn agents",()=>_(),{kind:"primary",icon:"plus"});r.addEventListener("change",()=>{e=r.value,m(ut.get())}),s.addEventListener("change",()=>{t=s.value,m(ut.get())});let c=[{label:"Agent",cell:x=>v("div.row.agent-cell",null,ci(ep[x.status]),v("div",null,v("strong.truncate",null,pn(x)),v("div.faint.truncate",null,yn(x.role))))},{label:"Model",cell:x=>v("div.row.agent-model",null,v("span.mono.truncate",null,x.model),x.tier==="orchestrator"?ot("orchestrator","accent"):null,x.model_source!=="default"?ot(x.model_source==="agent"?"own":"role"):null)},{label:"Work",cell:x=>x.work?v("div.row.agent-work",null,ot(x.work.kind,tp[x.work.kind]),v("span.truncate",null,Kt(x.work.prompt,56))):v("span.faint",null,"\u2014")},{label:"Tokens",num:!0,cell:x=>ws(x.usage.input_tokens+x.usage.output_tokens)},{label:"",width:"1%",cell:x=>h(x)}];n.append(v("div.view-head",null,v("h1",null,"Agents"),v("div.row.agent-filters",null,r,s,a)),zt(null,o,{flush:!0}));let l=ut.watch(m);return()=>l();function h(x){return v("div.row.agent-actions",null,We("",f=>{f.stopPropagation(),_n("agents",x.cid)},{kind:"ghost",size:"sm",icon:"conversations",title:"Chat"}),x.status==="working"?We("",f=>{f.stopPropagation(),dt(()=>Oe.cancel(x.cid),"Work cancelled")},{kind:"ghost",size:"sm",icon:"ban",title:"Cancel work"}):null,We("",f=>{f.stopPropagation(),dt(()=>Oe.stop(x.cid),"Session stopped")},{kind:"ghost",size:"sm",icon:"stop",title:"Stop session"}))}function m(x){if(u(x),!x.length){Ue(o,ht("agents","No agents connected","Spawn agents to fill the roster.",We("Spawn agents",()=>_(),{kind:"primary",icon:"plus"})));return}let f=x.filter(d=>(e===""||d.role===e)&&(t==="all"||d.status===t));if(!f.length){Ue(o,ht("search","No agents match","Clear the filters to see the roster.",We("Clear filters",g,{kind:"ghost"})));return}Ue(o,v("div.table-scroll",null,rr(c,f,d=>_n("agents",d.cid))))}function u(x){let f=[...new Set(x.map(d=>d.role))].sort();f.join(`
`)!==i.join(`
`)&&(i=f,e&&!i.includes(e)&&(e=""),r.replaceChildren(new Option("All roles",""),...i.map(d=>new Option(yn(d),d))),r.value=e)}function g(){e="",t="all",r.value="",s.value="all",m(ut.get())}function _(){let x=fn(i.map(T=>({value:T,label:T}))),f=Zt({type:"number",min:1,max:Xl,value:1}),d=fn([{value:"",label:"Default for the role"}]),S=We("Spawn",()=>{w()},{kind:"primary",icon:"plus"}),P=()=>{},w=async()=>{let T=Math.trunc(Number(f.value));if(!x.value){nn("Pick a role","error");return}if(!(T>=1&&T<=Xl)){nn(`Count must be 1 to ${Xl}`,"error");return}S.disabled=!0;let A=await dt(()=>Oe.spawnAgents(x.value,T,d.value||void 0),"Spawning\u2026");S.disabled=!1,A!==void 0&&(P(),nr())};dt(()=>Oe.models()).then(T=>{T&&d.append(...T.models.map(A=>new Option(`${A.provider}/${A.id}`,`${A.provider}/${A.id}`)))}),P=Cn("Spawn agents",v("div.stack",null,gt("Role",x),gt("Count",f),gt("Model",d),v("div.row.modal-actions",null,S)))}}function Zu(n,e){let t=Number(e[0]);return e[0]&&Number.isFinite(t)?$u(n,t):Yu(n)}var np=300,Ku="3";function Ju(n,e){let t=v("select.board-filter",{"aria-label":"Filter by author"}),i=v("input.board-search",{type:"search",placeholder:"Search subject or body","aria-label":"Search the board"}),r=v("div.board-feed");n.append(v("div.view-head",null,v("h1",null,"Board"),v("div.actions.board-actions",null,t,i)),r);let s=[],o=!1,a=!1,c=!0,l,h="",m="",u=P=>ut.get().filter(w=>w.cid===P.agent_cid).map(pn)[0]??`agent ${P.agent_cid}`,g=()=>{let P=new Map;for(let T of s){let A=String(T.agent_cid);P.has(A)||P.set(A,u(T))}let w=Array.from(P).sort((T,A)=>T[1].localeCompare(A[1]));h&&!P.has(h)&&(h=""),Ue(t,v("option",{value:""},"All authors"),w.map(([T,A])=>v("option",{value:T},A))),t.value=h},_=()=>{for(let P of r.querySelectorAll(".board-body")){let w=P.nextElementSibling;w instanceof HTMLElement&&P.scrollHeight>P.clientHeight+1&&(w.hidden=!1)}},x=()=>{if(!o)return;if(a)return Ue(r,ht("alert","Could not load the board."));if(g(),!s.length)return Ue(r,ht("board","The board is empty."));let P=[...s].sort((A,D)=>D.created_at-A.created_at),w=m.trim().toLowerCase(),T=P.map((A,D)=>({p:A,no:P.length-D})).filter(({p:A})=>!h||String(A.agent_cid)===h).filter(({p:A})=>!w||`${A.subject}
${A.body}`.toLowerCase().includes(w));if(!T.length)return Ue(r,ht("board","Nothing on the board matches."));Ue(r,T.map(({p:A,no:D})=>ip(A,D,u(A)))),_()},f=async()=>{try{s=await Oe.board(),a=!1}catch{a=!0}o=!0,c&&x()},S=[bt("board",()=>{window.clearTimeout(l),l=window.setTimeout(()=>{f()},np)}),ut.watch(()=>x())];return t.addEventListener("change",()=>{h=t.value,x()}),i.addEventListener("input",()=>{m=i.value,x()}),f(),()=>{c=!1,window.clearTimeout(l),S.forEach(P=>P())}}function ip(n,e,t){let i=n.body.trim(),r=i&&i!==n.subject.trim()?v("p.board-body",null,i):null;r&&r.style.setProperty("-webkit-line-clamp",Ku);let s=v("button.board-more",{type:"button",hidden:!0,onclick:()=>{if(!r)return;let o=r.classList.toggle("open");r.style.setProperty("-webkit-line-clamp",o?"none":Ku),s.textContent=o?"Show less":"Show more"}},"Show more");return v("article.board-entry",null,v("span.board-no",null,`p${e}`),v("div.board-matter",null,v("div.board-by",null,v("a.board-author",{href:vn("agents",n.agent_cid)},t),v("span.board-role",null,yn(n.role))),v("h3.board-subject",null,n.subject),r,r?s:null),v("time.board-time",null,Wt(n.created_at)))}var rp=300;function ju(n,e){let t=v("div.convo-list"),i=v("div.thread");n.append(v("div.view-head",null,v("h1",null,"Conversations")),v("div.convo",null,t,i));let r=[],s=!1,o=!1,a=!0,c,l=e[0]?sp(e[0]):void 0,h=f=>{let d=ut.get().find(S=>S.cid===f);return d?pn(d):`agent ${f}`},m=f=>{l=f,u(),_n("conversations",f)},u=()=>{if(!s)return;if(o){Ue(t,ht("alert","Could not load conversations.")),Ue(i);return}let f=op(r);if(!f.length){Ue(t,ht("conversations","No conversations yet.")),Ue(i);return}let d=f.find(S=>S.key===l)??f[0];d&&(l=d.key,Ue(t,f.map(S=>ap(S,S.key===d.key,h,()=>m(S.key)))),Ue(i,...lp(d,h)),i.scrollTop=i.scrollHeight)},g=async()=>{try{r=await Oe.conversations(),o=!1}catch{o=!0}s=!0,a&&u()},x=[bt("conversation",()=>{window.clearTimeout(c),c=window.setTimeout(()=>{g()},rp)}),ut.watch(()=>u())];return g(),()=>{a=!1,window.clearTimeout(c),x.forEach(f=>f())}}function sp(n){return n.split("-").map(Number).sort((e,t)=>e-t).join("-")}function op(n){let e=new Map,t=[...n].sort((i,r)=>i.created_at-r.created_at);for(let i of t){let r=Math.min(i.from_cid,i.to_cid),s=Math.max(i.from_cid,i.to_cid),o=`${r}-${s}`,a=e.get(o)??{key:o,a:r,b:s,messages:[],last:i};a.messages.push(i),a.last=i,e.set(o,a)}return Array.from(e.values()).sort((i,r)=>r.last.created_at-i.last.created_at)}function ql(n,e){return v("a.agent-link",{href:vn("agents",n)},e(n))}function ap(n,e,t,i){let r=v("button.convo-item",{type:"button","aria-current":e?"true":null},v("span.convo-names",null,`${t(n.a)} \xB7 ${t(n.b)}`),v("span.convo-last",null,Kt(n.last.text,90)),v("span.convo-meta",null,`${Wt(n.last.created_at)} \xB7 ${n.messages.length} messages`));return r.addEventListener("click",i),r}function lp(n,e){let t=n.messages[0]?.from_cid??n.a;return[v("div.thread-head",null,ql(n.a,e)," \xB7 ",ql(n.b,e)),...n.messages.map(i=>cp(i,i.from_cid===t?"left":"right",e))]}function cp(n,e,t){return v("div.bubble-row",{class:e},v("div.bubble",null,v("div.bubble-meta",null,ql(n.from_cid,t),v("span.faint",null,Wt(n.created_at)),n.delivered?null:ot("undelivered","warn")),v("p.bubble-text",null,n.text)))}var eh=n=>/owner|await/i.test(n.state);function up(n){return eh(n)?"warn":/carr|pass|approv/i.test(n.state)?"ok":/reject|fail|den/i.test(n.state)?"danger":/open/i.test(n.state)?"info":"muted"}var hp={yes:"ok",no:"danger",abstain:"muted"},$l=(n,e)=>`${e>0?n/e*100:0}%`;function th(n,e){let t=v("div.stack");n.append(v("div.view-head",null,v("h1",null,"Council")),t);let i=!0,r=async()=>{try{let o=await Oe.motions();if(!i)return;Ue(t,o.length?o.map(a=>dp(a,()=>{r()})):ht("council","No motions."))}catch(o){i&&Ue(t,ht("alert","Council unreachable",o instanceof Error?o.message:String(o)))}},s=bt("motion",()=>{r()});return r(),()=>{i=!1,s()}}function dp(n,e){let t=n.tally,i=v("div.tally",{role:"img","aria-label":`${t.yes} yes, ${t.no} no, ${t.abstain} abstain of ${t.seats}`},v("span.tally-seg.yes",{style:`width:${$l(t.yes,t.seats)}`}),v("span.tally-seg.no",{style:`width:${$l(t.no,t.seats)}`}),v("span.tally-seg.abstain",{style:`width:${$l(t.abstain,t.seats)}`})),r=n.votes?.length?v("ul.votes",null,n.votes.map(o=>v("li",null,v("span.muted",null,yn(o.role)),ot(o.vote,hp[o.vote]??"muted"),v("span",null,o.reason)))):null,s=eh(n)?v("div.motion-actions",null,We("Reject",()=>Qu(n,"reject",e),{kind:"danger",icon:"x"}),We("Approve",()=>Qu(n,"approve",e),{kind:"primary",icon:"check"})):null;return zt(v("div.motion-head",null,ot(n.kind,"accent"),ot(n.state,up(n),!0),v("span.faint",null,Wt(n.created_at))),v("div.stack",null,v("p.motion-text",null,n.text),i,v("div.tally-legend.faint",null,`${t.yes} yes \xB7 ${t.no} no \xB7 ${t.abstain} abstain \xB7 ${t.voted}/${t.seats}`),r,s))}function Qu(n,e,t){let i=e==="approve"?"Approve":"Reject",r=Kn({rows:3,placeholder:"Reason"}),s=We(i,()=>{s.disabled=!0,dt(()=>Oe.decide(n.id,e,r.value.trim()),`Motion ${e==="approve"?"approved":"rejected"}`).then(a=>{a?(o(),t()):s.disabled=!1})},{kind:e==="approve"?"primary":"danger"}),o=Cn(`${i} motion`,v("div.stack",null,v("p.muted",null,n.text),r,v("div.motion-actions",null,We("Cancel",()=>o(),{kind:"ghost"}),s)))}var $i={LEFT:0,MIDDLE:1,RIGHT:2,ROTATE:0,DOLLY:1,PAN:2},Yi={ROTATE:0,PAN:1,DOLLY_PAN:2,DOLLY_ROTATE:3},Lh=0,Ac=1,kh=2;var eo=1,Pa=2,os=3,Zi=0,Rn=1,qn=2,mi=0,as=1,ls=2,Rc=3,Cc=4,Dh=5;var pr=100,Nh=101,Uh=102,Fh=103,Oh=104,Bh=200,zh=201,Hh=202,Vh=203,Pc=204,Ic=205,Gh=206,Wh=207,Xh=208,qh=209,$h=210,Yh=211,Zh=212,Kh=213,Jh=214,Jo=0,jo=1,Qo=2,qr=3,ea=4,ta=5,na=6,ia=7,Lc=0,jh=1,Qh=2,ni=0,kc=1,Dc=2,Nc=3,Uc=4,Fc=5,Oc=6,Bc=7;var zc=300,Ki=301,mr=302,Ia=303,La=304,to=306,ur=1e3,hi=1001,ra=1002,hn=1003,ed=1004;var no=1005;var gn=1006,ka=1007;var Ji=1008;var Ln=1009,Hc=1010,Vc=1011,cs=1012,Da=1013,ii=1014,ri=1015,si=1016,Na=1017,Ua=1018,us=1020,Gc=35902,Wc=35899,Xc=1021,qc=1022,$n=1023,di=1026,ji=1027,$c=1028,Fa=1029,Qi=1030,Oa=1031;var Ba=1033,io=33776,ro=33777,so=33778,oo=33779,za=35840,Ha=35841,Va=35842,Ga=35843,Wa=36196,Xa=37492,qa=37496,$a=37488,Ya=37489,ao=37490,Za=37491,Ka=37808,Ja=37809,ja=37810,Qa=37811,el=37812,tl=37813,nl=37814,il=37815,rl=37816,sl=37817,ol=37818,al=37819,ll=37820,cl=37821,ul=36492,hl=36494,dl=36495,fl=36283,pl=36284,lo=36285,ml=36286;var Is=2300,sa=2301,Zo=2302,xc=2303,vc=2400,_c=2401,yc=2402;var td=3200;var gl=0,nd=1,Ci="",Ht="srgb",Ls="srgb-linear",ks="linear",Mt="srgb";var Ko=7680;var id=519,rd=512,sd=513,od=514,xl=515,ad=516,ld=517,vl=518,cd=519,ud=35044;var Yc="300 es",ei=2e3,$r=2001;function fp(n){for(let e=n.length-1;e>=0;--e)if(n[e]>=65535)return!0;return!1}function pp(n){return ArrayBuffer.isView(n)&&!(n instanceof DataView)}function Ds(n){return document.createElementNS("http://www.w3.org/1999/xhtml",n)}function hd(){let n=Ds("canvas");return n.style.display="block",n}var nh={},Yr=null;function Zc(...n){let e="THREE."+n.shift();Yr?Yr("log",e,...n):console.log(e,...n)}function dd(n){let e=n[0];if(typeof e=="string"&&e.startsWith("TSL:")){let t=n[1];t&&t.isStackTrace?n[0]+=" "+t.getLocation():n[1]='Stack trace not available. Enable "THREE.Node.captureStackTrace" to capture stack traces.'}return n}function Ge(...n){n=dd(n);let e="THREE."+n.shift();if(Yr)Yr("warn",e,...n);else{let t=n[0];t&&t.isStackTrace?console.warn(t.getError(e)):console.warn(e,...n)}}function Xe(...n){n=dd(n);let e="THREE."+n.shift();if(Yr)Yr("error",e,...n);else{let t=n[0];t&&t.isStackTrace?console.error(t.getError(e)):console.error(e,...n)}}function cr(...n){let e=n.join(" ");e in nh||(nh[e]=!0,Ge(...n))}function fd(n,e,t){return new Promise(function(i,r){function s(){switch(n.clientWaitSync(e,n.SYNC_FLUSH_COMMANDS_BIT,0)){case n.WAIT_FAILED:r();break;case n.TIMEOUT_EXPIRED:setTimeout(s,t);break;default:i()}}setTimeout(s,t)})}var pd={[Jo]:jo,[Qo]:na,[ea]:ia,[qr]:ta,[jo]:Jo,[na]:Qo,[ia]:ea,[ta]:qr},ti=class{addEventListener(e,t){this._listeners===void 0&&(this._listeners={});let i=this._listeners;i[e]===void 0&&(i[e]=[]),i[e].indexOf(t)===-1&&i[e].push(t)}hasEventListener(e,t){let i=this._listeners;return i===void 0?!1:i[e]!==void 0&&i[e].indexOf(t)!==-1}removeEventListener(e,t){let i=this._listeners;if(i===void 0)return;let r=i[e];if(r!==void 0){let s=r.indexOf(t);s!==-1&&r.splice(s,1)}}dispatchEvent(e){let t=this._listeners;if(t===void 0)return;let i=t[e.type];if(i!==void 0){e.target=this;let r=i.slice(0);for(let s=0,o=r.length;s<o;s++)r[s].call(this,e);e.target=null}}},bn=["00","01","02","03","04","05","06","07","08","09","0a","0b","0c","0d","0e","0f","10","11","12","13","14","15","16","17","18","19","1a","1b","1c","1d","1e","1f","20","21","22","23","24","25","26","27","28","29","2a","2b","2c","2d","2e","2f","30","31","32","33","34","35","36","37","38","39","3a","3b","3c","3d","3e","3f","40","41","42","43","44","45","46","47","48","49","4a","4b","4c","4d","4e","4f","50","51","52","53","54","55","56","57","58","59","5a","5b","5c","5d","5e","5f","60","61","62","63","64","65","66","67","68","69","6a","6b","6c","6d","6e","6f","70","71","72","73","74","75","76","77","78","79","7a","7b","7c","7d","7e","7f","80","81","82","83","84","85","86","87","88","89","8a","8b","8c","8d","8e","8f","90","91","92","93","94","95","96","97","98","99","9a","9b","9c","9d","9e","9f","a0","a1","a2","a3","a4","a5","a6","a7","a8","a9","aa","ab","ac","ad","ae","af","b0","b1","b2","b3","b4","b5","b6","b7","b8","b9","ba","bb","bc","bd","be","bf","c0","c1","c2","c3","c4","c5","c6","c7","c8","c9","ca","cb","cc","cd","ce","cf","d0","d1","d2","d3","d4","d5","d6","d7","d8","d9","da","db","dc","dd","de","df","e0","e1","e2","e3","e4","e5","e6","e7","e8","e9","ea","eb","ec","ed","ee","ef","f0","f1","f2","f3","f4","f5","f6","f7","f8","f9","fa","fb","fc","fd","fe","ff"],ih=1234567,Wr=Math.PI/180,Zr=180/Math.PI;function hs(){let n=Math.random()*4294967295|0,e=Math.random()*4294967295|0,t=Math.random()*4294967295|0,i=Math.random()*4294967295|0;return(bn[n&255]+bn[n>>8&255]+bn[n>>16&255]+bn[n>>24&255]+"-"+bn[e&255]+bn[e>>8&255]+"-"+bn[e>>16&15|64]+bn[e>>24&255]+"-"+bn[t&63|128]+bn[t>>8&255]+"-"+bn[t>>16&255]+bn[t>>24&255]+bn[i&255]+bn[i>>8&255]+bn[i>>16&255]+bn[i>>24&255]).toLowerCase()}function it(n,e,t){return Math.max(e,Math.min(t,n))}function Kc(n,e){return(n%e+e)%e}function mp(n,e,t,i,r){return i+(n-e)*(r-i)/(t-e)}function gp(n,e,t){return n!==e?(t-n)/(e-n):0}function Ps(n,e,t){return(1-t)*n+t*e}function xp(n,e,t,i){return Ps(n,e,1-Math.exp(-t*i))}function vp(n,e=1){return e-Math.abs(Kc(n,e*2)-e)}function _p(n,e,t){return n<=e?0:n>=t?1:(n=(n-e)/(t-e),n*n*(3-2*n))}function yp(n,e,t){return n<=e?0:n>=t?1:(n=(n-e)/(t-e),n*n*n*(n*(n*6-15)+10))}function bp(n,e){return n+Math.floor(Math.random()*(e-n+1))}function Mp(n,e){return n+Math.random()*(e-n)}function Sp(n){return n*(.5-Math.random())}function wp(n){n!==void 0&&(ih=n);let e=ih+=1831565813;return e=Math.imul(e^e>>>15,e|1),e^=e+Math.imul(e^e>>>7,e|61),((e^e>>>14)>>>0)/4294967296}function Ep(n){return n*Wr}function Tp(n){return n*Zr}function Ap(n){return n>0&&Number.isInteger(n)&&2**Math.round(Math.log2(n))===n}function Rp(n){return Math.pow(2,Math.ceil(Math.log(n)/Math.LN2))}function Cp(n){return Math.pow(2,Math.floor(Math.log(n)/Math.LN2))}function Pp(n,e,t,i,r){let s=Math.cos,o=Math.sin,a=s(t/2),c=o(t/2),l=s((e+i)/2),h=o((e+i)/2),m=s((e-i)/2),u=o((e-i)/2),g=s((i-e)/2),_=o((i-e)/2);switch(r){case"XYX":n.set(a*h,c*m,c*u,a*l);break;case"YZY":n.set(c*u,a*h,c*m,a*l);break;case"ZXZ":n.set(c*m,c*u,a*h,a*l);break;case"XZX":n.set(a*h,c*_,c*g,a*l);break;case"YXY":n.set(c*g,a*h,c*_,a*l);break;case"ZYZ":n.set(c*_,c*g,a*h,a*l);break;default:Ge("MathUtils: .setQuaternionFromProperEuler() encountered an unknown order: "+r)}}function Gr(n,e){switch(e.constructor){case Float32Array:return n;case Uint32Array:return n/4294967295;case Uint16Array:return n/65535;case Uint8Array:case Uint8ClampedArray:return n/255;case Int32Array:return Math.max(n/2147483647,-1);case Int16Array:return Math.max(n/32767,-1);case Int8Array:return Math.max(n/127,-1);default:throw new Error("THREE.MathUtils: Invalid component type.")}}function Tn(n,e){switch(e.constructor){case Float32Array:return n;case Uint32Array:return Math.round(n*4294967295);case Uint16Array:return Math.round(n*65535);case Uint8Array:case Uint8ClampedArray:return Math.round(n*255);case Int32Array:return Math.round(n*2147483647);case Int16Array:return Math.round(n*32767);case Int8Array:return Math.round(n*127);default:throw new Error("THREE.MathUtils: Invalid component type.")}}var Jc={DEG2RAD:Wr,RAD2DEG:Zr,generateUUID:hs,clamp:it,euclideanModulo:Kc,mapLinear:mp,inverseLerp:gp,lerp:Ps,damp:xp,pingpong:vp,smoothstep:_p,smootherstep:yp,randInt:bp,randFloat:Mp,randFloatSpread:Sp,seededRandom:wp,degToRad:Ep,radToDeg:Tp,isPowerOfTwo:Ap,ceilPowerOfTwo:Rp,floorPowerOfTwo:Cp,setQuaternionFromProperEuler:Pp,normalize:Tn,denormalize:Gr},He=class n{static{n.prototype.isVector2=!0}constructor(e=0,t=0){this.x=e,this.y=t}get width(){return this.x}set width(e){this.x=e}get height(){return this.y}set height(e){this.y=e}set(e,t){return this.x=e,this.y=t,this}setScalar(e){return this.x=e,this.y=e,this}setX(e){return this.x=e,this}setY(e){return this.y=e,this}setComponent(e,t){switch(e){case 0:this.x=t;break;case 1:this.y=t;break;default:throw new Error("THREE.Vector2: index is out of range: "+e)}return this}getComponent(e){switch(e){case 0:return this.x;case 1:return this.y;default:throw new Error("THREE.Vector2: index is out of range: "+e)}}clone(){return new this.constructor(this.x,this.y)}copy(e){return this.x=e.x,this.y=e.y,this}add(e){return this.x+=e.x,this.y+=e.y,this}addScalar(e){return this.x+=e,this.y+=e,this}addVectors(e,t){return this.x=e.x+t.x,this.y=e.y+t.y,this}addScaledVector(e,t){return this.x+=e.x*t,this.y+=e.y*t,this}sub(e){return this.x-=e.x,this.y-=e.y,this}subScalar(e){return this.x-=e,this.y-=e,this}subVectors(e,t){return this.x=e.x-t.x,this.y=e.y-t.y,this}multiply(e){return this.x*=e.x,this.y*=e.y,this}multiplyScalar(e){return this.x*=e,this.y*=e,this}divide(e){return this.x/=e.x,this.y/=e.y,this}divideScalar(e){return this.multiplyScalar(1/e)}applyMatrix3(e){let t=this.x,i=this.y,r=e.elements;return this.x=r[0]*t+r[3]*i+r[6],this.y=r[1]*t+r[4]*i+r[7],this}min(e){return this.x=Math.min(this.x,e.x),this.y=Math.min(this.y,e.y),this}max(e){return this.x=Math.max(this.x,e.x),this.y=Math.max(this.y,e.y),this}clamp(e,t){return this.x=it(this.x,e.x,t.x),this.y=it(this.y,e.y,t.y),this}clampScalar(e,t){return this.x=it(this.x,e,t),this.y=it(this.y,e,t),this}clampLength(e,t){let i=this.length();return this.divideScalar(i||1).multiplyScalar(it(i,e,t))}floor(){return this.x=Math.floor(this.x),this.y=Math.floor(this.y),this}ceil(){return this.x=Math.ceil(this.x),this.y=Math.ceil(this.y),this}round(){return this.x=Math.round(this.x),this.y=Math.round(this.y),this}roundToZero(){return this.x=Math.trunc(this.x),this.y=Math.trunc(this.y),this}negate(){return this.x=-this.x,this.y=-this.y,this}dot(e){return this.x*e.x+this.y*e.y}cross(e){return this.x*e.y-this.y*e.x}lengthSq(){return this.x*this.x+this.y*this.y}length(){return Math.sqrt(this.x*this.x+this.y*this.y)}manhattanLength(){return Math.abs(this.x)+Math.abs(this.y)}normalize(){return this.divideScalar(this.length()||1)}angle(){return Math.atan2(-this.y,-this.x)+Math.PI}angleTo(e){let t=Math.sqrt(this.lengthSq()*e.lengthSq());if(t===0)return Math.PI/2;let i=this.dot(e)/t;return Math.acos(it(i,-1,1))}distanceTo(e){return Math.sqrt(this.distanceToSquared(e))}distanceToSquared(e){let t=this.x-e.x,i=this.y-e.y;return t*t+i*i}manhattanDistanceTo(e){return Math.abs(this.x-e.x)+Math.abs(this.y-e.y)}setLength(e){return this.normalize().multiplyScalar(e)}lerp(e,t){return this.x+=(e.x-this.x)*t,this.y+=(e.y-this.y)*t,this}lerpVectors(e,t,i){return this.x=e.x+(t.x-e.x)*i,this.y=e.y+(t.y-e.y)*i,this}equals(e){return e.x===this.x&&e.y===this.y}fromArray(e,t=0){return this.x=e[t],this.y=e[t+1],this}toArray(e=[],t=0){return e[t]=this.x,e[t+1]=this.y,e}fromBufferAttribute(e,t){return this.x=e.getX(t),this.y=e.getY(t),this}rotateAround(e,t){let i=Math.cos(t),r=Math.sin(t),s=this.x-e.x,o=this.y-e.y;return this.x=s*i-o*r+e.x,this.y=s*r+o*i+e.y,this}random(){return this.x=Math.random(),this.y=Math.random(),this}*[Symbol.iterator](){yield this.x,yield this.y}},Fn=class{constructor(e=0,t=0,i=0,r=1){this.isQuaternion=!0,this._x=e,this._y=t,this._z=i,this._w=r}static slerpFlat(e,t,i,r,s,o,a){let c=i[r+0],l=i[r+1],h=i[r+2],m=i[r+3],u=s[o+0],g=s[o+1],_=s[o+2],x=s[o+3];if(m!==x||c!==u||l!==g||h!==_){let f=c*u+l*g+h*_+m*x;f<0&&(u=-u,g=-g,_=-_,x=-x,f=-f);let d=1-a;if(f<.9995){let S=Math.acos(f),P=Math.sin(S);d=Math.sin(d*S)/P,a=Math.sin(a*S)/P,c=c*d+u*a,l=l*d+g*a,h=h*d+_*a,m=m*d+x*a}else{c=c*d+u*a,l=l*d+g*a,h=h*d+_*a,m=m*d+x*a;let S=1/Math.sqrt(c*c+l*l+h*h+m*m);c*=S,l*=S,h*=S,m*=S}}e[t]=c,e[t+1]=l,e[t+2]=h,e[t+3]=m}static multiplyQuaternionsFlat(e,t,i,r,s,o){let a=i[r],c=i[r+1],l=i[r+2],h=i[r+3],m=s[o],u=s[o+1],g=s[o+2],_=s[o+3];return e[t]=a*_+h*m+c*g-l*u,e[t+1]=c*_+h*u+l*m-a*g,e[t+2]=l*_+h*g+a*u-c*m,e[t+3]=h*_-a*m-c*u-l*g,e}get x(){return this._x}set x(e){this._x=e,this._onChangeCallback()}get y(){return this._y}set y(e){this._y=e,this._onChangeCallback()}get z(){return this._z}set z(e){this._z=e,this._onChangeCallback()}get w(){return this._w}set w(e){this._w=e,this._onChangeCallback()}set(e,t,i,r){return this._x=e,this._y=t,this._z=i,this._w=r,this._onChangeCallback(),this}clone(){return new this.constructor(this._x,this._y,this._z,this._w)}copy(e){return this._x=e.x,this._y=e.y,this._z=e.z,this._w=e.w,this._onChangeCallback(),this}setFromEuler(e,t=!0){let i=e._x,r=e._y,s=e._z,o=e._order,a=Math.cos,c=Math.sin,l=a(i/2),h=a(r/2),m=a(s/2),u=c(i/2),g=c(r/2),_=c(s/2);switch(o){case"XYZ":this._x=u*h*m+l*g*_,this._y=l*g*m-u*h*_,this._z=l*h*_+u*g*m,this._w=l*h*m-u*g*_;break;case"YXZ":this._x=u*h*m+l*g*_,this._y=l*g*m-u*h*_,this._z=l*h*_-u*g*m,this._w=l*h*m+u*g*_;break;case"ZXY":this._x=u*h*m-l*g*_,this._y=l*g*m+u*h*_,this._z=l*h*_+u*g*m,this._w=l*h*m-u*g*_;break;case"ZYX":this._x=u*h*m-l*g*_,this._y=l*g*m+u*h*_,this._z=l*h*_-u*g*m,this._w=l*h*m+u*g*_;break;case"YZX":this._x=u*h*m+l*g*_,this._y=l*g*m+u*h*_,this._z=l*h*_-u*g*m,this._w=l*h*m-u*g*_;break;case"XZY":this._x=u*h*m-l*g*_,this._y=l*g*m-u*h*_,this._z=l*h*_+u*g*m,this._w=l*h*m+u*g*_;break;default:Ge("Quaternion: .setFromEuler() encountered an unknown order: "+o)}return t===!0&&this._onChangeCallback(),this}setFromAxisAngle(e,t){let i=t/2,r=Math.sin(i);return this._x=e.x*r,this._y=e.y*r,this._z=e.z*r,this._w=Math.cos(i),this._onChangeCallback(),this}setFromRotationMatrix(e){let t=e.elements,i=t[0],r=t[4],s=t[8],o=t[1],a=t[5],c=t[9],l=t[2],h=t[6],m=t[10],u=i+a+m;if(u>0){let g=.5/Math.sqrt(u+1);this._w=.25/g,this._x=(h-c)*g,this._y=(s-l)*g,this._z=(o-r)*g}else if(i>a&&i>m){let g=2*Math.sqrt(1+i-a-m);this._w=(h-c)/g,this._x=.25*g,this._y=(r+o)/g,this._z=(s+l)/g}else if(a>m){let g=2*Math.sqrt(1+a-i-m);this._w=(s-l)/g,this._x=(r+o)/g,this._y=.25*g,this._z=(c+h)/g}else{let g=2*Math.sqrt(1+m-i-a);this._w=(o-r)/g,this._x=(s+l)/g,this._y=(c+h)/g,this._z=.25*g}return this._onChangeCallback(),this}setFromUnitVectors(e,t){let i=e.dot(t)+1;return i<1e-8?(i=0,Math.abs(e.x)>Math.abs(e.z)?(this._x=-e.y,this._y=e.x,this._z=0,this._w=i):(this._x=0,this._y=-e.z,this._z=e.y,this._w=i)):(this._x=e.y*t.z-e.z*t.y,this._y=e.z*t.x-e.x*t.z,this._z=e.x*t.y-e.y*t.x,this._w=i),this.normalize()}angleTo(e){return 2*Math.acos(Math.abs(it(this.dot(e),-1,1)))}rotateTowards(e,t){let i=this.angleTo(e);if(i===0)return this;let r=Math.min(1,t/i);return this.slerp(e,r),this}identity(){return this.set(0,0,0,1)}invert(){return this.conjugate()}conjugate(){return this._x*=-1,this._y*=-1,this._z*=-1,this._onChangeCallback(),this}dot(e){return this._x*e._x+this._y*e._y+this._z*e._z+this._w*e._w}lengthSq(){return this._x*this._x+this._y*this._y+this._z*this._z+this._w*this._w}length(){return Math.sqrt(this._x*this._x+this._y*this._y+this._z*this._z+this._w*this._w)}normalize(){let e=this.length();return e===0?(this._x=0,this._y=0,this._z=0,this._w=1):(e=1/e,this._x=this._x*e,this._y=this._y*e,this._z=this._z*e,this._w=this._w*e),this._onChangeCallback(),this}multiply(e){return this.multiplyQuaternions(this,e)}premultiply(e){return this.multiplyQuaternions(e,this)}multiplyQuaternions(e,t){let i=e._x,r=e._y,s=e._z,o=e._w,a=t._x,c=t._y,l=t._z,h=t._w;return this._x=i*h+o*a+r*l-s*c,this._y=r*h+o*c+s*a-i*l,this._z=s*h+o*l+i*c-r*a,this._w=o*h-i*a-r*c-s*l,this._onChangeCallback(),this}slerp(e,t){let i=e._x,r=e._y,s=e._z,o=e._w,a=this.dot(e);a<0&&(i=-i,r=-r,s=-s,o=-o,a=-a);let c=1-t;if(a<.9995){let l=Math.acos(a),h=Math.sin(l);c=Math.sin(c*l)/h,t=Math.sin(t*l)/h,this._x=this._x*c+i*t,this._y=this._y*c+r*t,this._z=this._z*c+s*t,this._w=this._w*c+o*t,this._onChangeCallback()}else this._x=this._x*c+i*t,this._y=this._y*c+r*t,this._z=this._z*c+s*t,this._w=this._w*c+o*t,this.normalize();return this}slerpQuaternions(e,t,i){return this.copy(e).slerp(t,i)}random(){let e=2*Math.PI*Math.random(),t=2*Math.PI*Math.random(),i=Math.random(),r=Math.sqrt(1-i),s=Math.sqrt(i);return this.set(r*Math.sin(e),r*Math.cos(e),s*Math.sin(t),s*Math.cos(t))}equals(e){return e._x===this._x&&e._y===this._y&&e._z===this._z&&e._w===this._w}fromArray(e,t=0){return this._x=e[t],this._y=e[t+1],this._z=e[t+2],this._w=e[t+3],this._onChangeCallback(),this}toArray(e=[],t=0){return e[t]=this._x,e[t+1]=this._y,e[t+2]=this._z,e[t+3]=this._w,e}fromBufferAttribute(e,t){return this._x=e.getX(t),this._y=e.getY(t),this._z=e.getZ(t),this._w=e.getW(t),this._onChangeCallback(),this}toJSON(){return this.toArray()}_onChange(e){return this._onChangeCallback=e,this}_onChangeCallback(){}*[Symbol.iterator](){yield this._x,yield this._y,yield this._z,yield this._w}},V=class n{static{n.prototype.isVector3=!0}constructor(e=0,t=0,i=0){this.x=e,this.y=t,this.z=i}set(e,t,i){return i===void 0&&(i=this.z),this.x=e,this.y=t,this.z=i,this}setScalar(e){return this.x=e,this.y=e,this.z=e,this}setX(e){return this.x=e,this}setY(e){return this.y=e,this}setZ(e){return this.z=e,this}setComponent(e,t){switch(e){case 0:this.x=t;break;case 1:this.y=t;break;case 2:this.z=t;break;default:throw new Error("THREE.Vector3: index is out of range: "+e)}return this}getComponent(e){switch(e){case 0:return this.x;case 1:return this.y;case 2:return this.z;default:throw new Error("THREE.Vector3: index is out of range: "+e)}}clone(){return new this.constructor(this.x,this.y,this.z)}copy(e){return this.x=e.x,this.y=e.y,this.z=e.z,this}add(e){return this.x+=e.x,this.y+=e.y,this.z+=e.z,this}addScalar(e){return this.x+=e,this.y+=e,this.z+=e,this}addVectors(e,t){return this.x=e.x+t.x,this.y=e.y+t.y,this.z=e.z+t.z,this}addScaledVector(e,t){return this.x+=e.x*t,this.y+=e.y*t,this.z+=e.z*t,this}sub(e){return this.x-=e.x,this.y-=e.y,this.z-=e.z,this}subScalar(e){return this.x-=e,this.y-=e,this.z-=e,this}subVectors(e,t){return this.x=e.x-t.x,this.y=e.y-t.y,this.z=e.z-t.z,this}multiply(e){return this.x*=e.x,this.y*=e.y,this.z*=e.z,this}multiplyScalar(e){return this.x*=e,this.y*=e,this.z*=e,this}multiplyVectors(e,t){return this.x=e.x*t.x,this.y=e.y*t.y,this.z=e.z*t.z,this}applyEuler(e){return this.applyQuaternion(rh.setFromEuler(e))}applyAxisAngle(e,t){return this.applyQuaternion(rh.setFromAxisAngle(e,t))}applyMatrix3(e){let t=this.x,i=this.y,r=this.z,s=e.elements;return this.x=s[0]*t+s[3]*i+s[6]*r,this.y=s[1]*t+s[4]*i+s[7]*r,this.z=s[2]*t+s[5]*i+s[8]*r,this}applyNormalMatrix(e){return this.applyMatrix3(e).normalize()}applyMatrix4(e){let t=this.x,i=this.y,r=this.z,s=e.elements,o=1/(s[3]*t+s[7]*i+s[11]*r+s[15]);return this.x=(s[0]*t+s[4]*i+s[8]*r+s[12])*o,this.y=(s[1]*t+s[5]*i+s[9]*r+s[13])*o,this.z=(s[2]*t+s[6]*i+s[10]*r+s[14])*o,this}applyQuaternion(e){let t=this.x,i=this.y,r=this.z,s=e.x,o=e.y,a=e.z,c=e.w,l=2*(o*r-a*i),h=2*(a*t-s*r),m=2*(s*i-o*t);return this.x=t+c*l+o*m-a*h,this.y=i+c*h+a*l-s*m,this.z=r+c*m+s*h-o*l,this}project(e){return this.applyMatrix4(e.matrixWorldInverse).applyMatrix4(e.projectionMatrix)}unproject(e){return this.applyMatrix4(e.projectionMatrixInverse).applyMatrix4(e.matrixWorld)}transformDirection(e){let t=this.x,i=this.y,r=this.z,s=e.elements;return this.x=s[0]*t+s[4]*i+s[8]*r,this.y=s[1]*t+s[5]*i+s[9]*r,this.z=s[2]*t+s[6]*i+s[10]*r,this.normalize()}divide(e){return this.x/=e.x,this.y/=e.y,this.z/=e.z,this}divideScalar(e){return this.multiplyScalar(1/e)}min(e){return this.x=Math.min(this.x,e.x),this.y=Math.min(this.y,e.y),this.z=Math.min(this.z,e.z),this}max(e){return this.x=Math.max(this.x,e.x),this.y=Math.max(this.y,e.y),this.z=Math.max(this.z,e.z),this}clamp(e,t){return this.x=it(this.x,e.x,t.x),this.y=it(this.y,e.y,t.y),this.z=it(this.z,e.z,t.z),this}clampScalar(e,t){return this.x=it(this.x,e,t),this.y=it(this.y,e,t),this.z=it(this.z,e,t),this}clampLength(e,t){let i=this.length();return this.divideScalar(i||1).multiplyScalar(it(i,e,t))}floor(){return this.x=Math.floor(this.x),this.y=Math.floor(this.y),this.z=Math.floor(this.z),this}ceil(){return this.x=Math.ceil(this.x),this.y=Math.ceil(this.y),this.z=Math.ceil(this.z),this}round(){return this.x=Math.round(this.x),this.y=Math.round(this.y),this.z=Math.round(this.z),this}roundToZero(){return this.x=Math.trunc(this.x),this.y=Math.trunc(this.y),this.z=Math.trunc(this.z),this}negate(){return this.x=-this.x,this.y=-this.y,this.z=-this.z,this}dot(e){return this.x*e.x+this.y*e.y+this.z*e.z}lengthSq(){return this.x*this.x+this.y*this.y+this.z*this.z}length(){return Math.sqrt(this.x*this.x+this.y*this.y+this.z*this.z)}manhattanLength(){return Math.abs(this.x)+Math.abs(this.y)+Math.abs(this.z)}normalize(){return this.divideScalar(this.length()||1)}setLength(e){return this.normalize().multiplyScalar(e)}lerp(e,t){return this.x+=(e.x-this.x)*t,this.y+=(e.y-this.y)*t,this.z+=(e.z-this.z)*t,this}lerpVectors(e,t,i){return this.x=e.x+(t.x-e.x)*i,this.y=e.y+(t.y-e.y)*i,this.z=e.z+(t.z-e.z)*i,this}cross(e){return this.crossVectors(this,e)}crossVectors(e,t){let i=e.x,r=e.y,s=e.z,o=t.x,a=t.y,c=t.z;return this.x=r*c-s*a,this.y=s*o-i*c,this.z=i*a-r*o,this}projectOnVector(e){let t=e.lengthSq();if(t===0)return this.set(0,0,0);let i=e.dot(this)/t;return this.copy(e).multiplyScalar(i)}projectOnPlane(e){return Yl.copy(this).projectOnVector(e),this.sub(Yl)}reflect(e){return this.sub(Yl.copy(e).multiplyScalar(2*this.dot(e)))}angleTo(e){let t=Math.sqrt(this.lengthSq()*e.lengthSq());if(t===0)return Math.PI/2;let i=this.dot(e)/t;return Math.acos(it(i,-1,1))}distanceTo(e){return Math.sqrt(this.distanceToSquared(e))}distanceToSquared(e){let t=this.x-e.x,i=this.y-e.y,r=this.z-e.z;return t*t+i*i+r*r}manhattanDistanceTo(e){return Math.abs(this.x-e.x)+Math.abs(this.y-e.y)+Math.abs(this.z-e.z)}setFromSpherical(e){return this.setFromSphericalCoords(e.radius,e.phi,e.theta)}setFromSphericalCoords(e,t,i){let r=Math.sin(t)*e;return this.x=r*Math.sin(i),this.y=Math.cos(t)*e,this.z=r*Math.cos(i),this}setFromCylindrical(e){return this.setFromCylindricalCoords(e.radius,e.theta,e.y)}setFromCylindricalCoords(e,t,i){return this.x=e*Math.sin(t),this.y=i,this.z=e*Math.cos(t),this}setFromMatrixPosition(e){let t=e.elements;return this.x=t[12],this.y=t[13],this.z=t[14],this}setFromMatrixScale(e){let t=this.setFromMatrixColumn(e,0).length(),i=this.setFromMatrixColumn(e,1).length(),r=this.setFromMatrixColumn(e,2).length();return this.x=t,this.y=i,this.z=r,this}setFromMatrixColumn(e,t){return this.fromArray(e.elements,t*4)}setFromMatrix3Column(e,t){return this.fromArray(e.elements,t*3)}setFromEuler(e){return this.x=e._x,this.y=e._y,this.z=e._z,this}setFromColor(e){return this.x=e.r,this.y=e.g,this.z=e.b,this}equals(e){return e.x===this.x&&e.y===this.y&&e.z===this.z}fromArray(e,t=0){return this.x=e[t],this.y=e[t+1],this.z=e[t+2],this}toArray(e=[],t=0){return e[t]=this.x,e[t+1]=this.y,e[t+2]=this.z,e}fromBufferAttribute(e,t){return this.x=e.getX(t),this.y=e.getY(t),this.z=e.getZ(t),this}random(){return this.x=Math.random(),this.y=Math.random(),this.z=Math.random(),this}randomDirection(){let e=Math.random()*Math.PI*2,t=Math.random()*2-1,i=Math.sqrt(1-t*t);return this.x=i*Math.cos(e),this.y=t,this.z=i*Math.sin(e),this}*[Symbol.iterator](){yield this.x,yield this.y,yield this.z}},Yl=new V,rh=new Fn,Ze=class n{static{n.prototype.isMatrix3=!0}constructor(e,t,i,r,s,o,a,c,l){this.elements=[1,0,0,0,1,0,0,0,1],e!==void 0&&this.set(e,t,i,r,s,o,a,c,l)}set(e,t,i,r,s,o,a,c,l){let h=this.elements;return h[0]=e,h[1]=r,h[2]=a,h[3]=t,h[4]=s,h[5]=c,h[6]=i,h[7]=o,h[8]=l,this}identity(){return this.set(1,0,0,0,1,0,0,0,1),this}copy(e){let t=this.elements,i=e.elements;return t[0]=i[0],t[1]=i[1],t[2]=i[2],t[3]=i[3],t[4]=i[4],t[5]=i[5],t[6]=i[6],t[7]=i[7],t[8]=i[8],this}extractBasis(e,t,i){return e.setFromMatrix3Column(this,0),t.setFromMatrix3Column(this,1),i.setFromMatrix3Column(this,2),this}setFromMatrix4(e){let t=e.elements;return this.set(t[0],t[4],t[8],t[1],t[5],t[9],t[2],t[6],t[10]),this}multiply(e){return this.multiplyMatrices(this,e)}premultiply(e){return this.multiplyMatrices(e,this)}multiplyMatrices(e,t){let i=e.elements,r=t.elements,s=this.elements,o=i[0],a=i[3],c=i[6],l=i[1],h=i[4],m=i[7],u=i[2],g=i[5],_=i[8],x=r[0],f=r[3],d=r[6],S=r[1],P=r[4],w=r[7],T=r[2],A=r[5],D=r[8];return s[0]=o*x+a*S+c*T,s[3]=o*f+a*P+c*A,s[6]=o*d+a*w+c*D,s[1]=l*x+h*S+m*T,s[4]=l*f+h*P+m*A,s[7]=l*d+h*w+m*D,s[2]=u*x+g*S+_*T,s[5]=u*f+g*P+_*A,s[8]=u*d+g*w+_*D,this}multiplyScalar(e){let t=this.elements;return t[0]*=e,t[3]*=e,t[6]*=e,t[1]*=e,t[4]*=e,t[7]*=e,t[2]*=e,t[5]*=e,t[8]*=e,this}determinant(){let e=this.elements,t=e[0],i=e[1],r=e[2],s=e[3],o=e[4],a=e[5],c=e[6],l=e[7],h=e[8];return t*o*h-t*a*l-i*s*h+i*a*c+r*s*l-r*o*c}invert(){let e=this.elements,t=e[0],i=e[1],r=e[2],s=e[3],o=e[4],a=e[5],c=e[6],l=e[7],h=e[8],m=h*o-a*l,u=a*c-h*s,g=l*s-o*c,_=t*m+i*u+r*g;if(_===0)return this.set(0,0,0,0,0,0,0,0,0);let x=1/_;return e[0]=m*x,e[1]=(r*l-h*i)*x,e[2]=(a*i-r*o)*x,e[3]=u*x,e[4]=(h*t-r*c)*x,e[5]=(r*s-a*t)*x,e[6]=g*x,e[7]=(i*c-l*t)*x,e[8]=(o*t-i*s)*x,this}transpose(){let e,t=this.elements;return e=t[1],t[1]=t[3],t[3]=e,e=t[2],t[2]=t[6],t[6]=e,e=t[5],t[5]=t[7],t[7]=e,this}getNormalMatrix(e){return this.setFromMatrix4(e).invert().transpose()}transposeIntoArray(e){let t=this.elements;return e[0]=t[0],e[1]=t[3],e[2]=t[6],e[3]=t[1],e[4]=t[4],e[5]=t[7],e[6]=t[2],e[7]=t[5],e[8]=t[8],this}setUvTransform(e,t,i,r,s,o,a){let c=Math.cos(s),l=Math.sin(s);return this.set(i*c,i*l,-i*(c*o+l*a)+o+e,-r*l,r*c,-r*(-l*o+c*a)+a+t,0,0,1),this}scale(e,t){return cr("Matrix3: .scale() is deprecated. Use .makeScale() instead."),this.premultiply(Zl.makeScale(e,t)),this}rotate(e){return cr("Matrix3: .rotate() is deprecated. Use .makeRotation() instead."),this.premultiply(Zl.makeRotation(-e)),this}translate(e,t){return cr("Matrix3: .translate() is deprecated. Use .makeTranslation() instead."),this.premultiply(Zl.makeTranslation(e,t)),this}makeTranslation(e,t){return e.isVector2?this.set(1,0,e.x,0,1,e.y,0,0,1):this.set(1,0,e,0,1,t,0,0,1),this}makeRotation(e){let t=Math.cos(e),i=Math.sin(e);return this.set(t,-i,0,i,t,0,0,0,1),this}makeScale(e,t){return this.set(e,0,0,0,t,0,0,0,1),this}equals(e){let t=this.elements,i=e.elements;for(let r=0;r<9;r++)if(t[r]!==i[r])return!1;return!0}fromArray(e,t=0){for(let i=0;i<9;i++)this.elements[i]=e[i+t];return this}toArray(e=[],t=0){let i=this.elements;return e[t]=i[0],e[t+1]=i[1],e[t+2]=i[2],e[t+3]=i[3],e[t+4]=i[4],e[t+5]=i[5],e[t+6]=i[6],e[t+7]=i[7],e[t+8]=i[8],e}clone(){return new this.constructor().fromArray(this.elements)}},Zl=new Ze,sh=new Ze().set(.4123908,.3575843,.1804808,.212639,.7151687,.0721923,.0193308,.1191948,.9505322),oh=new Ze().set(3.2409699,-1.5373832,-.4986108,-.9692436,1.8759675,.0415551,.0556301,-.203977,1.0569715);function Ip(){let n={enabled:!0,workingColorSpace:Ls,spaces:{},convert:function(r,s,o){return this.enabled===!1||s===o||!s||!o||(this.spaces[s].transfer===Mt&&(r.r=Ti(r.r),r.g=Ti(r.g),r.b=Ti(r.b)),this.spaces[s].primaries!==this.spaces[o].primaries&&(r.applyMatrix3(this.spaces[s].toXYZ),r.applyMatrix3(this.spaces[o].fromXYZ)),this.spaces[o].transfer===Mt&&(r.r=Xr(r.r),r.g=Xr(r.g),r.b=Xr(r.b))),r},workingToColorSpace:function(r,s){return this.convert(r,this.workingColorSpace,s)},colorSpaceToWorking:function(r,s){return this.convert(r,s,this.workingColorSpace)},getPrimaries:function(r){return this.spaces[r].primaries},getTransfer:function(r){return r===Ci?ks:this.spaces[r].transfer},getToneMappingMode:function(r){return this.spaces[r].outputColorSpaceConfig.toneMappingMode||"standard"},getLuminanceCoefficients:function(r,s=this.workingColorSpace){return r.fromArray(this.spaces[s].luminanceCoefficients)},define:function(r){Object.assign(this.spaces,r)},_getMatrix:function(r,s,o){return r.copy(this.spaces[s].toXYZ).multiply(this.spaces[o].fromXYZ)},_getDrawingBufferColorSpace:function(r){return this.spaces[r].outputColorSpaceConfig.drawingBufferColorSpace},_getUnpackColorSpace:function(r=this.workingColorSpace){return this.spaces[r].workingColorSpaceConfig.unpackColorSpace},fromWorkingColorSpace:function(r,s){return cr("ColorManagement: .fromWorkingColorSpace() has been renamed to .workingToColorSpace()."),n.workingToColorSpace(r,s)},toWorkingColorSpace:function(r,s){return cr("ColorManagement: .toWorkingColorSpace() has been renamed to .colorSpaceToWorking()."),n.colorSpaceToWorking(r,s)}},e=[.64,.33,.3,.6,.15,.06],t=[.2126,.7152,.0722],i=[.3127,.329];return n.define({[Ls]:{primaries:e,whitePoint:i,transfer:ks,toXYZ:sh,fromXYZ:oh,luminanceCoefficients:t,workingColorSpaceConfig:{unpackColorSpace:Ht},outputColorSpaceConfig:{drawingBufferColorSpace:Ht}},[Ht]:{primaries:e,whitePoint:i,transfer:Mt,toXYZ:sh,fromXYZ:oh,luminanceCoefficients:t,outputColorSpaceConfig:{drawingBufferColorSpace:Ht}}}),n}var lt=Ip();function Ti(n){return n<.04045?n*.0773993808:Math.pow(n*.9478672986+.0521327014,2.4)}function Xr(n){return n<.0031308?n*12.92:1.055*Math.pow(n,.41666)-.055}var Pr,oa=class{static getDataURL(e,t="image/png"){if(/^data:/i.test(e.src)||typeof HTMLCanvasElement>"u")return e.src;let i;if(e instanceof HTMLCanvasElement)i=e;else{Pr===void 0&&(Pr=Ds("canvas")),Pr.width=e.width,Pr.height=e.height;let r=Pr.getContext("2d");e instanceof ImageData?r.putImageData(e,0,0):r.drawImage(e,0,0,e.width,e.height),i=Pr}return i.toDataURL(t)}static sRGBToLinear(e){if(typeof HTMLImageElement<"u"&&e instanceof HTMLImageElement||typeof HTMLCanvasElement<"u"&&e instanceof HTMLCanvasElement||typeof ImageBitmap<"u"&&e instanceof ImageBitmap){let t=Ds("canvas");t.width=e.width,t.height=e.height;let i=t.getContext("2d");i.drawImage(e,0,0,e.width,e.height);let r=i.getImageData(0,0,e.width,e.height),s=r.data;for(let o=0;o<s.length;o++)s[o]=Ti(s[o]/255)*255;return i.putImageData(r,0,0),t}else if(e.data){let t=e.data.slice(0);for(let i=0;i<t.length;i++)t instanceof Uint8Array||t instanceof Uint8ClampedArray?t[i]=Math.floor(Ti(t[i]/255)*255):t[i]=Ti(t[i]);return{data:t,width:e.width,height:e.height}}else return Ge("ImageUtils.sRGBToLinear(): Unsupported image type. No color space conversion applied."),e}},Lp=0,Kr=class{constructor(e=null){this.isTextureSource=!0,Object.defineProperty(this,"id",{value:Lp++}),this.uuid=hs(),this.data=e,this.dataReady=!0,this.version=0}getSize(e){let t=this.data;return typeof HTMLVideoElement<"u"&&t instanceof HTMLVideoElement?e.set(t.videoWidth,t.videoHeight,0):typeof VideoFrame<"u"&&t instanceof VideoFrame?e.set(t.displayWidth,t.displayHeight,0):t!==null?e.set(t.width,t.height,t.depth||0):e.set(0,0,0),e}set needsUpdate(e){e===!0&&this.version++}toJSON(e){let t=e===void 0||typeof e=="string";if(!t&&e.images[this.uuid]!==void 0)return e.images[this.uuid];let i={uuid:this.uuid,url:""},r=this.data;if(r!==null){let s;if(Array.isArray(r)){s=[];for(let o=0,a=r.length;o<a;o++)r[o].isDataTexture?s.push(Kl(r[o].image)):s.push(Kl(r[o]))}else s=Kl(r);i.url=s}return t||(e.images[this.uuid]=i),i}};function Kl(n){return typeof HTMLImageElement<"u"&&n instanceof HTMLImageElement||typeof HTMLCanvasElement<"u"&&n instanceof HTMLCanvasElement||typeof ImageBitmap<"u"&&n instanceof ImageBitmap?oa.getDataURL(n):n.data?{data:Array.from(n.data),width:n.width,height:n.height,type:n.data.constructor.name}:(Ge("Texture: Unable to serialize Texture."),{})}var kp=0,Jl=new V,An=class n extends ti{constructor(e=n.DEFAULT_IMAGE,t=n.DEFAULT_MAPPING,i=hi,r=hi,s=gn,o=Ji,a=$n,c=Ln,l=n.DEFAULT_ANISOTROPY,h=Ci){super(),this.isTexture=!0,Object.defineProperty(this,"id",{value:kp++}),this.uuid=hs(),this.name="",this.source=new Kr(e),this.mipmaps=[],this.mapping=t,this.channel=0,this.wrapS=i,this.wrapT=r,this.magFilter=s,this.minFilter=o,this.anisotropy=l,this.format=a,this.internalFormat=null,this.type=c,this.offset=new He(0,0),this.repeat=new He(1,1),this.center=new He(0,0),this.rotation=0,this.matrixAutoUpdate=!0,this.matrix=new Ze,this.generateMipmaps=!0,this.premultiplyAlpha=!1,this.flipY=!0,this.unpackAlignment=4,this.colorSpace=h,this.userData={},this.updateRanges=[],this.version=0,this.onUpdate=null,this.renderTarget=null,this.isRenderTargetTexture=!1,this.isArrayTexture=!!(e&&e.depth&&e.depth>1),this.pmremVersion=0,this.normalized=!1}get width(){return this.source.getSize(Jl).x}get height(){return this.source.getSize(Jl).y}get depth(){return this.source.getSize(Jl).z}get image(){return this.source.data}set image(e){this.source.data=e}updateMatrix(){this.matrix.setUvTransform(this.offset.x,this.offset.y,this.repeat.x,this.repeat.y,this.rotation,this.center.x,this.center.y)}addUpdateRange(e,t){this.updateRanges.push({start:e,count:t})}clearUpdateRanges(){this.updateRanges.length=0}clone(){return new this.constructor().copy(this)}copy(e){return this.name=e.name,this.source=e.source,this.mipmaps=e.mipmaps.slice(0),this.mapping=e.mapping,this.channel=e.channel,this.wrapS=e.wrapS,this.wrapT=e.wrapT,this.magFilter=e.magFilter,this.minFilter=e.minFilter,this.anisotropy=e.anisotropy,this.format=e.format,this.internalFormat=e.internalFormat,this.type=e.type,this.normalized=e.normalized,this.offset.copy(e.offset),this.repeat.copy(e.repeat),this.center.copy(e.center),this.rotation=e.rotation,this.matrixAutoUpdate=e.matrixAutoUpdate,this.matrix.copy(e.matrix),this.generateMipmaps=e.generateMipmaps,this.premultiplyAlpha=e.premultiplyAlpha,this.flipY=e.flipY,this.unpackAlignment=e.unpackAlignment,this.colorSpace=e.colorSpace,this.renderTarget=e.renderTarget,this.isRenderTargetTexture=e.isRenderTargetTexture,this.isArrayTexture=e.isArrayTexture,this.userData=JSON.parse(JSON.stringify(e.userData)),this.needsUpdate=!0,this}setValues(e){for(let t in e){let i=e[t];if(i===void 0){Ge(`Texture.setValues(): parameter '${t}' has value of undefined.`);continue}let r=this[t];if(r===void 0){Ge(`Texture.setValues(): property '${t}' does not exist.`);continue}r&&i&&r.isVector2&&i.isVector2||r&&i&&r.isVector3&&i.isVector3||r&&i&&r.isMatrix3&&i.isMatrix3?r.copy(i):this[t]=i}}toJSON(e){let t=e===void 0||typeof e=="string";if(!t&&e.textures[this.uuid]!==void 0)return e.textures[this.uuid];let i={metadata:{version:4.7,type:"Texture",generator:"Texture.toJSON"},uuid:this.uuid,name:this.name,image:this.source.toJSON(e).uuid,mapping:this.mapping,channel:this.channel,repeat:[this.repeat.x,this.repeat.y],offset:[this.offset.x,this.offset.y],center:[this.center.x,this.center.y],rotation:this.rotation,wrap:[this.wrapS,this.wrapT],format:this.format,internalFormat:this.internalFormat,type:this.type,normalized:this.normalized,colorSpace:this.colorSpace,minFilter:this.minFilter,magFilter:this.magFilter,anisotropy:this.anisotropy,flipY:this.flipY,generateMipmaps:this.generateMipmaps,premultiplyAlpha:this.premultiplyAlpha,unpackAlignment:this.unpackAlignment};return Object.keys(this.userData).length>0&&(i.userData=this.userData),t||(e.textures[this.uuid]=i),i}dispose(){this.dispatchEvent({type:"dispose"})}transformUv(e){if(this.mapping!==zc)return e;if(e.applyMatrix3(this.matrix),e.x<0||e.x>1)switch(this.wrapS){case ur:e.x=e.x-Math.floor(e.x);break;case hi:e.x=e.x<0?0:1;break;case ra:Math.abs(Math.floor(e.x)%2)===1?e.x=Math.ceil(e.x)-e.x:e.x=e.x-Math.floor(e.x);break}if(e.y<0||e.y>1)switch(this.wrapT){case ur:e.y=e.y-Math.floor(e.y);break;case hi:e.y=e.y<0?0:1;break;case ra:Math.abs(Math.floor(e.y)%2)===1?e.y=Math.ceil(e.y)-e.y:e.y=e.y-Math.floor(e.y);break}return this.flipY&&(e.y=1-e.y),e}set needsUpdate(e){e===!0&&(this.version++,this.source.needsUpdate=!0)}set needsPMREMUpdate(e){e===!0&&this.pmremVersion++}};An.DEFAULT_IMAGE=null;An.DEFAULT_MAPPING=zc;An.DEFAULT_ANISOTROPY=1;var Ot=class n{static{n.prototype.isVector4=!0}constructor(e=0,t=0,i=0,r=1){this.x=e,this.y=t,this.z=i,this.w=r}get width(){return this.z}set width(e){this.z=e}get height(){return this.w}set height(e){this.w=e}set(e,t,i,r){return this.x=e,this.y=t,this.z=i,this.w=r,this}setScalar(e){return this.x=e,this.y=e,this.z=e,this.w=e,this}setX(e){return this.x=e,this}setY(e){return this.y=e,this}setZ(e){return this.z=e,this}setW(e){return this.w=e,this}setComponent(e,t){switch(e){case 0:this.x=t;break;case 1:this.y=t;break;case 2:this.z=t;break;case 3:this.w=t;break;default:throw new Error("THREE.Vector4: index is out of range: "+e)}return this}getComponent(e){switch(e){case 0:return this.x;case 1:return this.y;case 2:return this.z;case 3:return this.w;default:throw new Error("THREE.Vector4: index is out of range: "+e)}}clone(){return new this.constructor(this.x,this.y,this.z,this.w)}copy(e){return this.x=e.x,this.y=e.y,this.z=e.z,this.w=e.w!==void 0?e.w:1,this}add(e){return this.x+=e.x,this.y+=e.y,this.z+=e.z,this.w+=e.w,this}addScalar(e){return this.x+=e,this.y+=e,this.z+=e,this.w+=e,this}addVectors(e,t){return this.x=e.x+t.x,this.y=e.y+t.y,this.z=e.z+t.z,this.w=e.w+t.w,this}addScaledVector(e,t){return this.x+=e.x*t,this.y+=e.y*t,this.z+=e.z*t,this.w+=e.w*t,this}sub(e){return this.x-=e.x,this.y-=e.y,this.z-=e.z,this.w-=e.w,this}subScalar(e){return this.x-=e,this.y-=e,this.z-=e,this.w-=e,this}subVectors(e,t){return this.x=e.x-t.x,this.y=e.y-t.y,this.z=e.z-t.z,this.w=e.w-t.w,this}multiply(e){return this.x*=e.x,this.y*=e.y,this.z*=e.z,this.w*=e.w,this}multiplyScalar(e){return this.x*=e,this.y*=e,this.z*=e,this.w*=e,this}applyMatrix4(e){let t=this.x,i=this.y,r=this.z,s=this.w,o=e.elements;return this.x=o[0]*t+o[4]*i+o[8]*r+o[12]*s,this.y=o[1]*t+o[5]*i+o[9]*r+o[13]*s,this.z=o[2]*t+o[6]*i+o[10]*r+o[14]*s,this.w=o[3]*t+o[7]*i+o[11]*r+o[15]*s,this}divide(e){return this.x/=e.x,this.y/=e.y,this.z/=e.z,this.w/=e.w,this}divideScalar(e){return this.multiplyScalar(1/e)}setAxisAngleFromQuaternion(e){this.w=2*Math.acos(e.w);let t=Math.sqrt(1-e.w*e.w);return t<1e-4?(this.x=1,this.y=0,this.z=0):(this.x=e.x/t,this.y=e.y/t,this.z=e.z/t),this}setAxisAngleFromRotationMatrix(e){let t,i,r,s,c=e.elements,l=c[0],h=c[4],m=c[8],u=c[1],g=c[5],_=c[9],x=c[2],f=c[6],d=c[10];if(Math.abs(h-u)<.01&&Math.abs(m-x)<.01&&Math.abs(_-f)<.01){if(Math.abs(h+u)<.1&&Math.abs(m+x)<.1&&Math.abs(_+f)<.1&&Math.abs(l+g+d-3)<.1)return this.set(1,0,0,0),this;t=Math.PI;let P=(l+1)/2,w=(g+1)/2,T=(d+1)/2,A=(h+u)/4,D=(m+x)/4,p=(_+f)/4;return P>w&&P>T?P<.01?(i=0,r=.707106781,s=.707106781):(i=Math.sqrt(P),r=A/i,s=D/i):w>T?w<.01?(i=.707106781,r=0,s=.707106781):(r=Math.sqrt(w),i=A/r,s=p/r):T<.01?(i=.707106781,r=.707106781,s=0):(s=Math.sqrt(T),i=D/s,r=p/s),this.set(i,r,s,t),this}let S=Math.sqrt((f-_)*(f-_)+(m-x)*(m-x)+(u-h)*(u-h));return Math.abs(S)<.001&&(S=1),this.x=(f-_)/S,this.y=(m-x)/S,this.z=(u-h)/S,this.w=Math.acos((l+g+d-1)/2),this}setFromMatrixPosition(e){let t=e.elements;return this.x=t[12],this.y=t[13],this.z=t[14],this.w=t[15],this}min(e){return this.x=Math.min(this.x,e.x),this.y=Math.min(this.y,e.y),this.z=Math.min(this.z,e.z),this.w=Math.min(this.w,e.w),this}max(e){return this.x=Math.max(this.x,e.x),this.y=Math.max(this.y,e.y),this.z=Math.max(this.z,e.z),this.w=Math.max(this.w,e.w),this}clamp(e,t){return this.x=it(this.x,e.x,t.x),this.y=it(this.y,e.y,t.y),this.z=it(this.z,e.z,t.z),this.w=it(this.w,e.w,t.w),this}clampScalar(e,t){return this.x=it(this.x,e,t),this.y=it(this.y,e,t),this.z=it(this.z,e,t),this.w=it(this.w,e,t),this}clampLength(e,t){let i=this.length();return this.divideScalar(i||1).multiplyScalar(it(i,e,t))}floor(){return this.x=Math.floor(this.x),this.y=Math.floor(this.y),this.z=Math.floor(this.z),this.w=Math.floor(this.w),this}ceil(){return this.x=Math.ceil(this.x),this.y=Math.ceil(this.y),this.z=Math.ceil(this.z),this.w=Math.ceil(this.w),this}round(){return this.x=Math.round(this.x),this.y=Math.round(this.y),this.z=Math.round(this.z),this.w=Math.round(this.w),this}roundToZero(){return this.x=Math.trunc(this.x),this.y=Math.trunc(this.y),this.z=Math.trunc(this.z),this.w=Math.trunc(this.w),this}negate(){return this.x=-this.x,this.y=-this.y,this.z=-this.z,this.w=-this.w,this}dot(e){return this.x*e.x+this.y*e.y+this.z*e.z+this.w*e.w}lengthSq(){return this.x*this.x+this.y*this.y+this.z*this.z+this.w*this.w}length(){return Math.sqrt(this.x*this.x+this.y*this.y+this.z*this.z+this.w*this.w)}manhattanLength(){return Math.abs(this.x)+Math.abs(this.y)+Math.abs(this.z)+Math.abs(this.w)}normalize(){return this.divideScalar(this.length()||1)}setLength(e){return this.normalize().multiplyScalar(e)}lerp(e,t){return this.x+=(e.x-this.x)*t,this.y+=(e.y-this.y)*t,this.z+=(e.z-this.z)*t,this.w+=(e.w-this.w)*t,this}lerpVectors(e,t,i){return this.x=e.x+(t.x-e.x)*i,this.y=e.y+(t.y-e.y)*i,this.z=e.z+(t.z-e.z)*i,this.w=e.w+(t.w-e.w)*i,this}equals(e){return e.x===this.x&&e.y===this.y&&e.z===this.z&&e.w===this.w}fromArray(e,t=0){return this.x=e[t],this.y=e[t+1],this.z=e[t+2],this.w=e[t+3],this}toArray(e=[],t=0){return e[t]=this.x,e[t+1]=this.y,e[t+2]=this.z,e[t+3]=this.w,e}fromBufferAttribute(e,t){return this.x=e.getX(t),this.y=e.getY(t),this.z=e.getZ(t),this.w=e.getW(t),this}random(){return this.x=Math.random(),this.y=Math.random(),this.z=Math.random(),this.w=Math.random(),this}*[Symbol.iterator](){yield this.x,yield this.y,yield this.z,yield this.w}},aa=class extends ti{constructor(e=1,t=1,i={}){super(),i=Object.assign({generateMipmaps:!1,internalFormat:null,minFilter:gn,depthBuffer:!0,stencilBuffer:!1,resolveColorBuffer:!0,resolveDepthBuffer:!0,resolveStencilBuffer:!0,storeMultisampledColorBuffer:!0,storeMultisampledDepthBuffer:!0,storeMultisampledStencilBuffer:!0,depthTexture:null,samples:0,count:1,depth:1,multiview:!1,useArrayDepthTexture:!1},i),this.isRenderTarget=!0,this.width=e,this.height=t,this.depth=i.depth,this.scissor=new Ot(0,0,e,t),this.scissorTest=!1,this.viewport=new Ot(0,0,e,t),this.textures=[];let r={width:e,height:t,depth:i.depth},s=new An(r),o=i.count;for(let a=0;a<o;a++)this.textures[a]=s.clone(),this.textures[a].isRenderTargetTexture=!0,this.textures[a].renderTarget=this;this._setTextureOptions(i),this.depthBuffer=i.depthBuffer,this.stencilBuffer=i.stencilBuffer,this.resolveColorBuffer=i.resolveColorBuffer,this.resolveDepthBuffer=i.resolveDepthBuffer,this.resolveStencilBuffer=i.resolveStencilBuffer,this.storeMultisampledColorBuffer=i.storeMultisampledColorBuffer,this.storeMultisampledDepthBuffer=i.storeMultisampledDepthBuffer,this.storeMultisampledStencilBuffer=i.storeMultisampledStencilBuffer,this._depthTexture=null,this.depthTexture=i.depthTexture,this.samples=i.samples,this.multiview=i.multiview,this.useArrayDepthTexture=i.useArrayDepthTexture}_setTextureOptions(e={}){let t={minFilter:gn,generateMipmaps:!1,flipY:!1,internalFormat:null};e.mapping!==void 0&&(t.mapping=e.mapping),e.wrapS!==void 0&&(t.wrapS=e.wrapS),e.wrapT!==void 0&&(t.wrapT=e.wrapT),e.wrapR!==void 0&&(t.wrapR=e.wrapR),e.magFilter!==void 0&&(t.magFilter=e.magFilter),e.minFilter!==void 0&&(t.minFilter=e.minFilter),e.format!==void 0&&(t.format=e.format),e.type!==void 0&&(t.type=e.type),e.anisotropy!==void 0&&(t.anisotropy=e.anisotropy),e.colorSpace!==void 0&&(t.colorSpace=e.colorSpace),e.flipY!==void 0&&(t.flipY=e.flipY),e.generateMipmaps!==void 0&&(t.generateMipmaps=e.generateMipmaps),e.internalFormat!==void 0&&(t.internalFormat=e.internalFormat);for(let i=0;i<this.textures.length;i++)this.textures[i].setValues(t)}get texture(){return this.textures[0]}set texture(e){this.textures[0]=e}set depthTexture(e){this._depthTexture!==null&&this._depthTexture.renderTarget===this&&(this._depthTexture.renderTarget=null),e!==null&&e.renderTarget===null&&(e.renderTarget=this),this._depthTexture=e}get depthTexture(){return this._depthTexture}setSize(e,t,i=1){if(this.width!==e||this.height!==t||this.depth!==i){this.width=e,this.height=t,this.depth=i;for(let r=0,s=this.textures.length;r<s;r++)this.textures[r].image.width=e,this.textures[r].image.height=t,this.textures[r].image.depth=i,this.textures[r].isData3DTexture!==!0&&(this.textures[r].isArrayTexture=this.textures[r].image.depth>1);this.dispose()}this.viewport.set(0,0,e,t),this.scissor.set(0,0,e,t)}clone(){return new this.constructor().copy(this)}copy(e){this.width=e.width,this.height=e.height,this.depth=e.depth,this.scissor.copy(e.scissor),this.scissorTest=e.scissorTest,this.viewport.copy(e.viewport),this.textures.length=0;for(let t=0,i=e.textures.length;t<i;t++){this.textures[t]=e.textures[t].clone(),this.textures[t].isRenderTargetTexture=!0,this.textures[t].renderTarget=this;let r=Object.assign({},e.textures[t].image);this.textures[t].source=new Kr(r)}if(this.depthBuffer=e.depthBuffer,this.stencilBuffer=e.stencilBuffer,this.resolveColorBuffer=e.resolveColorBuffer,this.resolveDepthBuffer=e.resolveDepthBuffer,this.resolveStencilBuffer=e.resolveStencilBuffer,this.storeMultisampledColorBuffer=e.storeMultisampledColorBuffer,this.storeMultisampledDepthBuffer=e.storeMultisampledDepthBuffer,this.storeMultisampledStencilBuffer=e.storeMultisampledStencilBuffer,e.depthTexture!==null)if(e.depthTexture.renderTarget===e){let t=e.depthTexture.clone();t.renderTarget=null,this.depthTexture=t}else this.depthTexture=e.depthTexture;return this.samples=e.samples,this.multiview=e.multiview,this.useArrayDepthTexture=e.useArrayDepthTexture,this}dispose(){this.dispatchEvent({type:"dispose"})}},In=class extends aa{constructor(e=1,t=1,i={}){super(e,t,i),this.isWebGLRenderTarget=!0}},Ns=class extends An{constructor(e=null,t=1,i=1,r=1){super(null),this.isDataArrayTexture=!0,this.image={data:e,width:t,height:i,depth:r},this.magFilter=hn,this.minFilter=hn,this.wrapR=hi,this.generateMipmaps=!1,this.flipY=!1,this.unpackAlignment=1,this.layerUpdates=new Set}copy(e){return super.copy(e),this.wrapR=e.wrapR,this}addLayerUpdate(e){this.layerUpdates.add(e)}clearLayerUpdates(){this.layerUpdates.clear()}};var la=class extends An{constructor(e=null,t=1,i=1,r=1){super(null),this.isData3DTexture=!0,this.image={data:e,width:t,height:i,depth:r},this.magFilter=hn,this.minFilter=hn,this.wrapR=hi,this.generateMipmaps=!1,this.flipY=!1,this.unpackAlignment=1}copy(e){return super.copy(e),this.wrapR=e.wrapR,this}};var It=class n{static{n.prototype.isMatrix4=!0}constructor(e,t,i,r,s,o,a,c,l,h,m,u,g,_,x,f){this.elements=[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],e!==void 0&&this.set(e,t,i,r,s,o,a,c,l,h,m,u,g,_,x,f)}set(e,t,i,r,s,o,a,c,l,h,m,u,g,_,x,f){let d=this.elements;return d[0]=e,d[4]=t,d[8]=i,d[12]=r,d[1]=s,d[5]=o,d[9]=a,d[13]=c,d[2]=l,d[6]=h,d[10]=m,d[14]=u,d[3]=g,d[7]=_,d[11]=x,d[15]=f,this}identity(){return this.set(1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1),this}clone(){return new n().fromArray(this.elements)}copy(e){let t=this.elements,i=e.elements;return t[0]=i[0],t[1]=i[1],t[2]=i[2],t[3]=i[3],t[4]=i[4],t[5]=i[5],t[6]=i[6],t[7]=i[7],t[8]=i[8],t[9]=i[9],t[10]=i[10],t[11]=i[11],t[12]=i[12],t[13]=i[13],t[14]=i[14],t[15]=i[15],this}copyPosition(e){let t=this.elements,i=e.elements;return t[12]=i[12],t[13]=i[13],t[14]=i[14],this}setFromMatrix3(e){let t=e.elements;return this.set(t[0],t[3],t[6],0,t[1],t[4],t[7],0,t[2],t[5],t[8],0,0,0,0,1),this}extractBasis(e,t,i){return this.determinantAffine()===0?(e.set(1,0,0),t.set(0,1,0),i.set(0,0,1),this):(e.setFromMatrixColumn(this,0),t.setFromMatrixColumn(this,1),i.setFromMatrixColumn(this,2),this)}makeBasis(e,t,i){return this.set(e.x,t.x,i.x,0,e.y,t.y,i.y,0,e.z,t.z,i.z,0,0,0,0,1),this}extractRotation(e){if(e.determinantAffine()===0)return this.identity();let t=this.elements,i=e.elements,r=1/Ir.setFromMatrixColumn(e,0).length(),s=1/Ir.setFromMatrixColumn(e,1).length(),o=1/Ir.setFromMatrixColumn(e,2).length();return t[0]=i[0]*r,t[1]=i[1]*r,t[2]=i[2]*r,t[3]=0,t[4]=i[4]*s,t[5]=i[5]*s,t[6]=i[6]*s,t[7]=0,t[8]=i[8]*o,t[9]=i[9]*o,t[10]=i[10]*o,t[11]=0,t[12]=0,t[13]=0,t[14]=0,t[15]=1,this}makeRotationFromEuler(e){let t=this.elements,i=e.x,r=e.y,s=e.z,o=Math.cos(i),a=Math.sin(i),c=Math.cos(r),l=Math.sin(r),h=Math.cos(s),m=Math.sin(s);if(e.order==="XYZ"){let u=o*h,g=o*m,_=a*h,x=a*m;t[0]=c*h,t[4]=-c*m,t[8]=l,t[1]=g+_*l,t[5]=u-x*l,t[9]=-a*c,t[2]=x-u*l,t[6]=_+g*l,t[10]=o*c}else if(e.order==="YXZ"){let u=c*h,g=c*m,_=l*h,x=l*m;t[0]=u+x*a,t[4]=_*a-g,t[8]=o*l,t[1]=o*m,t[5]=o*h,t[9]=-a,t[2]=g*a-_,t[6]=x+u*a,t[10]=o*c}else if(e.order==="ZXY"){let u=c*h,g=c*m,_=l*h,x=l*m;t[0]=u-x*a,t[4]=-o*m,t[8]=_+g*a,t[1]=g+_*a,t[5]=o*h,t[9]=x-u*a,t[2]=-o*l,t[6]=a,t[10]=o*c}else if(e.order==="ZYX"){let u=o*h,g=o*m,_=a*h,x=a*m;t[0]=c*h,t[4]=_*l-g,t[8]=u*l+x,t[1]=c*m,t[5]=x*l+u,t[9]=g*l-_,t[2]=-l,t[6]=a*c,t[10]=o*c}else if(e.order==="YZX"){let u=o*c,g=o*l,_=a*c,x=a*l;t[0]=c*h,t[4]=x-u*m,t[8]=_*m+g,t[1]=m,t[5]=o*h,t[9]=-a*h,t[2]=-l*h,t[6]=g*m+_,t[10]=u-x*m}else if(e.order==="XZY"){let u=o*c,g=o*l,_=a*c,x=a*l;t[0]=c*h,t[4]=-m,t[8]=l*h,t[1]=u*m+x,t[5]=o*h,t[9]=g*m-_,t[2]=_*m-g,t[6]=a*h,t[10]=x*m+u}return t[3]=0,t[7]=0,t[11]=0,t[12]=0,t[13]=0,t[14]=0,t[15]=1,this}makeRotationFromQuaternion(e){return this.compose(Dp,e,Np)}lookAt(e,t,i){let r=this.elements;return Dn.subVectors(e,t),Dn.lengthSq()===0&&(Dn.z=1),Dn.normalize(),Ui.crossVectors(i,Dn),Ui.lengthSq()===0&&(Math.abs(i.z)===1?Dn.x+=1e-4:Dn.z+=1e-4,Dn.normalize(),Ui.crossVectors(i,Dn)),Ui.normalize(),So.crossVectors(Dn,Ui),r[0]=Ui.x,r[4]=So.x,r[8]=Dn.x,r[1]=Ui.y,r[5]=So.y,r[9]=Dn.y,r[2]=Ui.z,r[6]=So.z,r[10]=Dn.z,this}multiply(e){return this.multiplyMatrices(this,e)}premultiply(e){return this.multiplyMatrices(e,this)}multiplyMatrices(e,t){let i=e.elements,r=t.elements,s=this.elements,o=i[0],a=i[4],c=i[8],l=i[12],h=i[1],m=i[5],u=i[9],g=i[13],_=i[2],x=i[6],f=i[10],d=i[14],S=i[3],P=i[7],w=i[11],T=i[15],A=r[0],D=r[4],p=r[8],M=r[12],y=r[1],C=r[5],k=r[9],L=r[13],U=r[2],F=r[6],H=r[10],q=r[14],ee=r[3],J=r[7],ne=r[11],ce=r[15];return s[0]=o*A+a*y+c*U+l*ee,s[4]=o*D+a*C+c*F+l*J,s[8]=o*p+a*k+c*H+l*ne,s[12]=o*M+a*L+c*q+l*ce,s[1]=h*A+m*y+u*U+g*ee,s[5]=h*D+m*C+u*F+g*J,s[9]=h*p+m*k+u*H+g*ne,s[13]=h*M+m*L+u*q+g*ce,s[2]=_*A+x*y+f*U+d*ee,s[6]=_*D+x*C+f*F+d*J,s[10]=_*p+x*k+f*H+d*ne,s[14]=_*M+x*L+f*q+d*ce,s[3]=S*A+P*y+w*U+T*ee,s[7]=S*D+P*C+w*F+T*J,s[11]=S*p+P*k+w*H+T*ne,s[15]=S*M+P*L+w*q+T*ce,this}multiplyScalar(e){let t=this.elements;return t[0]*=e,t[4]*=e,t[8]*=e,t[12]*=e,t[1]*=e,t[5]*=e,t[9]*=e,t[13]*=e,t[2]*=e,t[6]*=e,t[10]*=e,t[14]*=e,t[3]*=e,t[7]*=e,t[11]*=e,t[15]*=e,this}determinant(){let e=this.elements,t=e[0],i=e[4],r=e[8],s=e[12],o=e[1],a=e[5],c=e[9],l=e[13],h=e[2],m=e[6],u=e[10],g=e[14],_=e[3],x=e[7],f=e[11],d=e[15],S=c*g-l*u,P=a*g-l*m,w=a*u-c*m,T=o*g-l*h,A=o*u-c*h,D=o*m-a*h;return t*(x*S-f*P+d*w)-i*(_*S-f*T+d*A)+r*(_*P-x*T+d*D)-s*(_*w-x*A+f*D)}determinantAffine(){let e=this.elements,t=e[0],i=e[4],r=e[8],s=e[1],o=e[5],a=e[9],c=e[2],l=e[6],h=e[10];return t*(o*h-a*l)-i*(s*h-a*c)+r*(s*l-o*c)}transpose(){let e=this.elements,t;return t=e[1],e[1]=e[4],e[4]=t,t=e[2],e[2]=e[8],e[8]=t,t=e[6],e[6]=e[9],e[9]=t,t=e[3],e[3]=e[12],e[12]=t,t=e[7],e[7]=e[13],e[13]=t,t=e[11],e[11]=e[14],e[14]=t,this}setPosition(e,t,i){let r=this.elements;return e.isVector3?(r[12]=e.x,r[13]=e.y,r[14]=e.z):(r[12]=e,r[13]=t,r[14]=i),this}invert(){let e=this.elements,t=e[0],i=e[1],r=e[2],s=e[3],o=e[4],a=e[5],c=e[6],l=e[7],h=e[8],m=e[9],u=e[10],g=e[11],_=e[12],x=e[13],f=e[14],d=e[15],S=t*a-i*o,P=t*c-r*o,w=t*l-s*o,T=i*c-r*a,A=i*l-s*a,D=r*l-s*c,p=h*x-m*_,M=h*f-u*_,y=h*d-g*_,C=m*f-u*x,k=m*d-g*x,L=u*d-g*f,U=S*L-P*k+w*C+T*y-A*M+D*p;if(U===0)return this.set(0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0);let F=1/U;return e[0]=(a*L-c*k+l*C)*F,e[1]=(r*k-i*L-s*C)*F,e[2]=(x*D-f*A+d*T)*F,e[3]=(u*A-m*D-g*T)*F,e[4]=(c*y-o*L-l*M)*F,e[5]=(t*L-r*y+s*M)*F,e[6]=(f*w-_*D-d*P)*F,e[7]=(h*D-u*w+g*P)*F,e[8]=(o*k-a*y+l*p)*F,e[9]=(i*y-t*k-s*p)*F,e[10]=(_*A-x*w+d*S)*F,e[11]=(m*w-h*A-g*S)*F,e[12]=(a*M-o*C-c*p)*F,e[13]=(t*C-i*M+r*p)*F,e[14]=(x*P-_*T-f*S)*F,e[15]=(h*T-m*P+u*S)*F,this}scale(e){let t=this.elements,i=e.x,r=e.y,s=e.z;return t[0]*=i,t[4]*=r,t[8]*=s,t[1]*=i,t[5]*=r,t[9]*=s,t[2]*=i,t[6]*=r,t[10]*=s,t[3]*=i,t[7]*=r,t[11]*=s,this}getMaxScaleOnAxis(){let e=this.elements,t=e[0]*e[0]+e[1]*e[1]+e[2]*e[2],i=e[4]*e[4]+e[5]*e[5]+e[6]*e[6],r=e[8]*e[8]+e[9]*e[9]+e[10]*e[10];return Math.sqrt(Math.max(t,i,r))}makeTranslation(e,t,i){return e.isVector3?this.set(1,0,0,e.x,0,1,0,e.y,0,0,1,e.z,0,0,0,1):this.set(1,0,0,e,0,1,0,t,0,0,1,i,0,0,0,1),this}makeRotationX(e){let t=Math.cos(e),i=Math.sin(e);return this.set(1,0,0,0,0,t,-i,0,0,i,t,0,0,0,0,1),this}makeRotationY(e){let t=Math.cos(e),i=Math.sin(e);return this.set(t,0,i,0,0,1,0,0,-i,0,t,0,0,0,0,1),this}makeRotationZ(e){let t=Math.cos(e),i=Math.sin(e);return this.set(t,-i,0,0,i,t,0,0,0,0,1,0,0,0,0,1),this}makeRotationAxis(e,t){let i=Math.cos(t),r=Math.sin(t),s=1-i,o=e.x,a=e.y,c=e.z,l=s*o,h=s*a;return this.set(l*o+i,l*a-r*c,l*c+r*a,0,l*a+r*c,h*a+i,h*c-r*o,0,l*c-r*a,h*c+r*o,s*c*c+i,0,0,0,0,1),this}makeScale(e,t,i){return this.set(e,0,0,0,0,t,0,0,0,0,i,0,0,0,0,1),this}makeShear(e,t,i,r,s,o){return this.set(1,i,s,0,e,1,o,0,t,r,1,0,0,0,0,1),this}compose(e,t,i){let r=this.elements,s=t._x,o=t._y,a=t._z,c=t._w,l=s+s,h=o+o,m=a+a,u=s*l,g=s*h,_=s*m,x=o*h,f=o*m,d=a*m,S=c*l,P=c*h,w=c*m,T=i.x,A=i.y,D=i.z;return r[0]=(1-(x+d))*T,r[1]=(g+w)*T,r[2]=(_-P)*T,r[3]=0,r[4]=(g-w)*A,r[5]=(1-(u+d))*A,r[6]=(f+S)*A,r[7]=0,r[8]=(_+P)*D,r[9]=(f-S)*D,r[10]=(1-(u+x))*D,r[11]=0,r[12]=e.x,r[13]=e.y,r[14]=e.z,r[15]=1,this}decompose(e,t,i){let r=this.elements;e.x=r[12],e.y=r[13],e.z=r[14];let s=this.determinantAffine();if(s===0)return i.set(1,1,1),t.identity(),this;let o=Ir.set(r[0],r[1],r[2]).length(),a=Ir.set(r[4],r[5],r[6]).length(),c=Ir.set(r[8],r[9],r[10]).length();s<0&&(o=-o),Jn.copy(this);let l=1/o,h=1/a,m=1/c;return Jn.elements[0]*=l,Jn.elements[1]*=l,Jn.elements[2]*=l,Jn.elements[4]*=h,Jn.elements[5]*=h,Jn.elements[6]*=h,Jn.elements[8]*=m,Jn.elements[9]*=m,Jn.elements[10]*=m,t.setFromRotationMatrix(Jn),i.x=o,i.y=a,i.z=c,this}makePerspective(e,t,i,r,s,o,a=ei,c=!1){let l=this.elements,h=2*s/(t-e),m=2*s/(i-r),u=(t+e)/(t-e),g=(i+r)/(i-r),_,x;if(c)_=s/(o-s),x=o*s/(o-s);else if(a===ei)_=-(o+s)/(o-s),x=-2*o*s/(o-s);else if(a===$r)_=-o/(o-s),x=-o*s/(o-s);else throw new Error("THREE.Matrix4.makePerspective(): Invalid coordinate system: "+a);return l[0]=h,l[4]=0,l[8]=u,l[12]=0,l[1]=0,l[5]=m,l[9]=g,l[13]=0,l[2]=0,l[6]=0,l[10]=_,l[14]=x,l[3]=0,l[7]=0,l[11]=-1,l[15]=0,this}makeOrthographic(e,t,i,r,s,o,a=ei,c=!1){let l=this.elements,h=2/(t-e),m=2/(i-r),u=-(t+e)/(t-e),g=-(i+r)/(i-r),_,x;if(c)_=1/(o-s),x=o/(o-s);else if(a===ei)_=-2/(o-s),x=-(o+s)/(o-s);else if(a===$r)_=-1/(o-s),x=-s/(o-s);else throw new Error("THREE.Matrix4.makeOrthographic(): Invalid coordinate system: "+a);return l[0]=h,l[4]=0,l[8]=0,l[12]=u,l[1]=0,l[5]=m,l[9]=0,l[13]=g,l[2]=0,l[6]=0,l[10]=_,l[14]=x,l[3]=0,l[7]=0,l[11]=0,l[15]=1,this}equals(e){let t=this.elements,i=e.elements;for(let r=0;r<16;r++)if(t[r]!==i[r])return!1;return!0}fromArray(e,t=0){for(let i=0;i<16;i++)this.elements[i]=e[i+t];return this}toArray(e=[],t=0){let i=this.elements;return e[t]=i[0],e[t+1]=i[1],e[t+2]=i[2],e[t+3]=i[3],e[t+4]=i[4],e[t+5]=i[5],e[t+6]=i[6],e[t+7]=i[7],e[t+8]=i[8],e[t+9]=i[9],e[t+10]=i[10],e[t+11]=i[11],e[t+12]=i[12],e[t+13]=i[13],e[t+14]=i[14],e[t+15]=i[15],e}},Ir=new V,Jn=new It,Dp=new V(0,0,0),Np=new V(1,1,1),Ui=new V,So=new V,Dn=new V,ah=new It,lh=new Fn,Ai=class n{constructor(e=0,t=0,i=0,r=n.DEFAULT_ORDER){this.isEuler=!0,this._x=e,this._y=t,this._z=i,this._order=r}get x(){return this._x}set x(e){this._x=e,this._onChangeCallback()}get y(){return this._y}set y(e){this._y=e,this._onChangeCallback()}get z(){return this._z}set z(e){this._z=e,this._onChangeCallback()}get order(){return this._order}set order(e){this._order=e,this._onChangeCallback()}set(e,t,i,r=this._order){return this._x=e,this._y=t,this._z=i,this._order=r,this._onChangeCallback(),this}clone(){return new this.constructor(this._x,this._y,this._z,this._order)}copy(e){return this._x=e._x,this._y=e._y,this._z=e._z,this._order=e._order,this._onChangeCallback(),this}setFromRotationMatrix(e,t=this._order,i=!0){let r=e.elements,s=r[0],o=r[4],a=r[8],c=r[1],l=r[5],h=r[9],m=r[2],u=r[6],g=r[10];switch(t){case"XYZ":this._y=Math.asin(it(a,-1,1)),Math.abs(a)<.9999999?(this._x=Math.atan2(-h,g),this._z=Math.atan2(-o,s)):(this._x=Math.atan2(u,l),this._z=0);break;case"YXZ":this._x=Math.asin(-it(h,-1,1)),Math.abs(h)<.9999999?(this._y=Math.atan2(a,g),this._z=Math.atan2(c,l)):(this._y=Math.atan2(-m,s),this._z=0);break;case"ZXY":this._x=Math.asin(it(u,-1,1)),Math.abs(u)<.9999999?(this._y=Math.atan2(-m,g),this._z=Math.atan2(-o,l)):(this._y=0,this._z=Math.atan2(c,s));break;case"ZYX":this._y=Math.asin(-it(m,-1,1)),Math.abs(m)<.9999999?(this._x=Math.atan2(u,g),this._z=Math.atan2(c,s)):(this._x=0,this._z=Math.atan2(-o,l));break;case"YZX":this._z=Math.asin(it(c,-1,1)),Math.abs(c)<.9999999?(this._x=Math.atan2(-h,l),this._y=Math.atan2(-m,s)):(this._x=0,this._y=Math.atan2(a,g));break;case"XZY":this._z=Math.asin(-it(o,-1,1)),Math.abs(o)<.9999999?(this._x=Math.atan2(u,l),this._y=Math.atan2(a,s)):(this._x=Math.atan2(-h,g),this._y=0);break;default:Ge("Euler: .setFromRotationMatrix() encountered an unknown order: "+t)}return this._order=t,i===!0&&this._onChangeCallback(),this}setFromQuaternion(e,t,i){return ah.makeRotationFromQuaternion(e),this.setFromRotationMatrix(ah,t,i)}setFromVector3(e,t=this._order){return this.set(e.x,e.y,e.z,t)}reorder(e){return lh.setFromEuler(this),this.setFromQuaternion(lh,e)}equals(e){return e._x===this._x&&e._y===this._y&&e._z===this._z&&e._order===this._order}fromArray(e){return this._x=e[0],this._y=e[1],this._z=e[2],e[3]!==void 0&&(this._order=e[3]),this._onChangeCallback(),this}toArray(e=[],t=0){return e[t]=this._x,e[t+1]=this._y,e[t+2]=this._z,e[t+3]=this._order,e}_onChange(e){return this._onChangeCallback=e,this}_onChangeCallback(){}*[Symbol.iterator](){yield this._x,yield this._y,yield this._z,yield this._order}};Ai.DEFAULT_ORDER="XYZ";var Jr=class{constructor(){this.mask=1}set(e){this.mask=(1<<e|0)>>>0}enable(e){this.mask|=1<<e|0}enableAll(){this.mask=-1}toggle(e){this.mask^=1<<e|0}disable(e){this.mask&=~(1<<e|0)}disableAll(){this.mask=0}test(e){return(this.mask&e.mask)!==0}isEnabled(e){return(this.mask&(1<<e|0))!==0}},Up=0,ch=new V,Lr=new Fn,yi=new It,wo=new V,Es=new V,Fp=new V,Op=new Fn,uh=new V(1,0,0),hh=new V(0,1,0),dh=new V(0,0,1),fh={type:"added"},Bp={type:"removed"},kr={type:"childadded",child:null},jl={type:"childremoved",child:null},dn=class n extends ti{constructor(){super(),this.isObject3D=!0,Object.defineProperty(this,"id",{value:Up++}),this.uuid=hs(),this.name="",this.type="Object3D",this.parent=null,this.children=[],this.up=n.DEFAULT_UP.clone();let e=new V,t=new Ai,i=new Fn,r=new V(1,1,1);function s(){i.setFromEuler(t,!1)}function o(){t.setFromQuaternion(i,void 0,!1)}t._onChange(s),i._onChange(o),Object.defineProperties(this,{position:{configurable:!0,enumerable:!0,value:e},rotation:{configurable:!0,enumerable:!0,value:t},quaternion:{configurable:!0,enumerable:!0,value:i},scale:{configurable:!0,enumerable:!0,value:r},modelViewMatrix:{value:new It},normalMatrix:{value:new Ze}}),this.matrix=new It,this.matrixWorld=new It,this.matrixAutoUpdate=n.DEFAULT_MATRIX_AUTO_UPDATE,this.matrixWorldAutoUpdate=n.DEFAULT_MATRIX_WORLD_AUTO_UPDATE,this.matrixWorldNeedsUpdate=!1,this.layers=new Jr,this.visible=!0,this.castShadow=!1,this.receiveShadow=!1,this.frustumCulled=!0,this.renderOrder=0,this.animations=[],this.customDepthMaterial=void 0,this.customDistanceMaterial=void 0,this.static=!1,this.userData={},this.pivot=null}onBeforeShadow(){}onAfterShadow(){}onBeforeRender(){}onAfterRender(){}applyMatrix4(e){this.matrixAutoUpdate&&this.updateMatrix(),this.matrix.premultiply(e),this.matrix.decompose(this.position,this.quaternion,this.scale)}applyQuaternion(e){return this.quaternion.premultiply(e),this}setRotationFromAxisAngle(e,t){this.quaternion.setFromAxisAngle(e,t)}setRotationFromEuler(e){this.quaternion.setFromEuler(e,!0)}setRotationFromMatrix(e){this.quaternion.setFromRotationMatrix(e)}setRotationFromQuaternion(e){this.quaternion.copy(e)}rotateOnAxis(e,t){return Lr.setFromAxisAngle(e,t),this.quaternion.multiply(Lr),this}rotateOnWorldAxis(e,t){return Lr.setFromAxisAngle(e,t),this.quaternion.premultiply(Lr),this}rotateX(e){return this.rotateOnAxis(uh,e)}rotateY(e){return this.rotateOnAxis(hh,e)}rotateZ(e){return this.rotateOnAxis(dh,e)}translateOnAxis(e,t){return ch.copy(e).applyQuaternion(this.quaternion),this.position.add(ch.multiplyScalar(t)),this}translateX(e){return this.translateOnAxis(uh,e)}translateY(e){return this.translateOnAxis(hh,e)}translateZ(e){return this.translateOnAxis(dh,e)}localToWorld(e){return this.updateWorldMatrix(!0,!1),e.applyMatrix4(this.matrixWorld)}worldToLocal(e){return this.updateWorldMatrix(!0,!1),e.applyMatrix4(yi.copy(this.matrixWorld).invert())}lookAt(e,t,i){e.isVector3?wo.copy(e):wo.set(e,t,i);let r=this.parent;this.updateWorldMatrix(!0,!1),Es.setFromMatrixPosition(this.matrixWorld),this.isCamera||this.isLight?yi.lookAt(Es,wo,this.up):yi.lookAt(wo,Es,this.up),this.quaternion.setFromRotationMatrix(yi),r&&(yi.extractRotation(r.matrixWorld),Lr.setFromRotationMatrix(yi),this.quaternion.premultiply(Lr.invert()))}add(e){if(arguments.length>1){for(let t=0;t<arguments.length;t++)this.add(arguments[t]);return this}return e===this?(Xe("Object3D.add: object can't be added as a child of itself.",e),this):(e&&e.isObject3D?(e.removeFromParent(),e.parent=this,this.children.push(e),e.dispatchEvent(fh),kr.child=e,this.dispatchEvent(kr),kr.child=null):Xe("Object3D.add: object not an instance of THREE.Object3D.",e),this)}remove(e){if(arguments.length>1){for(let i=0;i<arguments.length;i++)this.remove(arguments[i]);return this}let t=this.children.indexOf(e);return t!==-1&&(e.parent=null,this.children.splice(t,1),e.dispatchEvent(Bp),jl.child=e,this.dispatchEvent(jl),jl.child=null),this}removeFromParent(){let e=this.parent;return e!==null&&e.remove(this),this}clear(){return this.remove(...this.children)}attach(e){return this.updateWorldMatrix(!0,!1),yi.copy(this.matrixWorld).invert(),e.parent!==null&&(e.parent.updateWorldMatrix(!0,!1),yi.multiply(e.parent.matrixWorld)),e.applyMatrix4(yi),e.removeFromParent(),e.parent=this,this.children.push(e),e.updateWorldMatrix(!1,!0),e.dispatchEvent(fh),kr.child=e,this.dispatchEvent(kr),kr.child=null,this}getObjectById(e){return this.getObjectByProperty("id",e)}getObjectByName(e){return this.getObjectByProperty("name",e)}getObjectByProperty(e,t){if(this[e]===t)return this;for(let i=0,r=this.children.length;i<r;i++){let o=this.children[i].getObjectByProperty(e,t);if(o!==void 0)return o}}getObjectsByProperty(e,t,i=[]){this[e]===t&&i.push(this);let r=this.children;for(let s=0,o=r.length;s<o;s++)r[s].getObjectsByProperty(e,t,i);return i}getWorldPosition(e){return this.updateWorldMatrix(!0,!1),e.setFromMatrixPosition(this.matrixWorld)}getWorldQuaternion(e){return this.updateWorldMatrix(!0,!1),this.matrixWorld.decompose(Es,e,Fp),e}getWorldScale(e){return this.updateWorldMatrix(!0,!1),this.matrixWorld.decompose(Es,Op,e),e}getWorldDirection(e){this.updateWorldMatrix(!0,!1);let t=this.matrixWorld.elements;return e.set(t[8],t[9],t[10]).normalize()}raycast(){}intersectsFrustum(){}traverse(e){e(this);let t=this.children;for(let i=0,r=t.length;i<r;i++)t[i].traverse(e)}traverseVisible(e){if(this.visible===!1)return;e(this);let t=this.children;for(let i=0,r=t.length;i<r;i++)t[i].traverseVisible(e)}traverseAncestors(e){let t=this.parent;t!==null&&(e(t),t.traverseAncestors(e))}updateMatrix(){this.matrix.compose(this.position,this.quaternion,this.scale);let e=this.pivot;if(e!==null){let t=e.x,i=e.y,r=e.z,s=this.matrix.elements;s[12]+=t-s[0]*t-s[4]*i-s[8]*r,s[13]+=i-s[1]*t-s[5]*i-s[9]*r,s[14]+=r-s[2]*t-s[6]*i-s[10]*r}this.matrixWorldNeedsUpdate=!0}updateMatrixWorld(e){this.matrixAutoUpdate&&this.updateMatrix(),(this.matrixWorldNeedsUpdate||e)&&(this.matrixWorldAutoUpdate===!0&&(this.parent===null?this.matrixWorld.copy(this.matrix):this.matrixWorld.multiplyMatrices(this.parent.matrixWorld,this.matrix)),this.matrixWorldNeedsUpdate=!1,e=!0);let t=this.children;for(let i=0,r=t.length;i<r;i++)t[i].updateMatrixWorld(e)}updateWorldMatrix(e,t,i=!1){let r=this.parent;if(e===!0&&r!==null&&r.updateWorldMatrix(!0,!1),this.matrixAutoUpdate&&this.updateMatrix(),(this.matrixWorldNeedsUpdate||i)&&(this.matrixWorldAutoUpdate===!0&&(this.parent===null?this.matrixWorld.copy(this.matrix):this.matrixWorld.multiplyMatrices(this.parent.matrixWorld,this.matrix)),this.matrixWorldNeedsUpdate=!1,i=!0),t===!0){let s=this.children;for(let o=0,a=s.length;o<a;o++)s[o].updateWorldMatrix(!1,!0,i)}}toJSON(e){let t=e===void 0||typeof e=="string",i={};t&&(e={geometries:{},materials:{},textures:{},images:{},shapes:{},skeletons:{},animations:{},nodes:{}},i.metadata={version:4.7,type:"Object",generator:"Object3D.toJSON"});let r={};r.uuid=this.uuid,r.type=this.type,r.name=this.name,r.castShadow=this.castShadow,r.receiveShadow=this.receiveShadow,r.visible=this.visible,r.frustumCulled=this.frustumCulled,r.renderOrder=this.renderOrder,r.static=this.static,r.matrixAutoUpdate=this.matrixAutoUpdate,Object.keys(this.userData).length>0&&(r.userData=this.userData),r.layers=this.layers.mask,r.matrix=this.matrix.toArray(),r.up=this.up.toArray(),this.pivot!==null&&(r.pivot=this.pivot.toArray()),this.morphTargetDictionary!==void 0&&(r.morphTargetDictionary=Object.assign({},this.morphTargetDictionary)),this.morphTargetInfluences!==void 0&&(r.morphTargetInfluences=this.morphTargetInfluences.slice()),this.isInstancedMesh&&(r.type="InstancedMesh",r.count=this.count,r.instanceMatrix=this.instanceMatrix.toJSON(),this.instanceColor!==null&&(r.instanceColor=this.instanceColor.toJSON())),this.isBatchedMesh&&(r.type="BatchedMesh",r.perObjectFrustumCulled=this.perObjectFrustumCulled,r.sortObjects=this.sortObjects,r.drawRanges=this._drawRanges,r.reservedRanges=this._reservedRanges,r.geometryInfo=this._geometryInfo.map(a=>({...a,boundingBox:a.boundingBox?a.boundingBox.toJSON():void 0,boundingSphere:a.boundingSphere?a.boundingSphere.toJSON():void 0})),r.instanceInfo=this._instanceInfo.map(a=>({...a})),r.availableInstanceIds=this._availableInstanceIds.slice(),r.availableGeometryIds=this._availableGeometryIds.slice(),r.nextIndexStart=this._nextIndexStart,r.nextVertexStart=this._nextVertexStart,r.geometryCount=this._geometryCount,r.maxInstanceCount=this._maxInstanceCount,r.maxVertexCount=this._maxVertexCount,r.maxIndexCount=this._maxIndexCount,r.geometryInitialized=this._geometryInitialized,r.matricesTexture=this._matricesTexture.toJSON(e),r.indirectTexture=this._indirectTexture.toJSON(e),this._colorsTexture!==null&&(r.colorsTexture=this._colorsTexture.toJSON(e)),this.boundingSphere!==null&&(r.boundingSphere=this.boundingSphere.toJSON()),this.boundingBox!==null&&(r.boundingBox=this.boundingBox.toJSON()));function s(a,c){return a[c.uuid]===void 0&&(a[c.uuid]=c.toJSON(e)),c.uuid}if(this.isScene)this.background&&(this.background.isColor?r.background=this.background.toJSON():this.background.isTexture&&(r.background=this.background.toJSON(e).uuid)),this.environment&&this.environment.isTexture&&this.environment.isRenderTargetTexture!==!0&&(r.environment=this.environment.toJSON(e).uuid);else if(this.isMesh||this.isLine||this.isPoints){r.geometry=s(e.geometries,this.geometry);let a=this.geometry.parameters;if(a!==void 0&&a.shapes!==void 0){let c=a.shapes;if(Array.isArray(c))for(let l=0,h=c.length;l<h;l++){let m=c[l];s(e.shapes,m)}else s(e.shapes,c)}}if(this.isSkinnedMesh&&(r.bindMode=this.bindMode,r.bindMatrix=this.bindMatrix.toArray(),this.skeleton!==void 0&&(s(e.skeletons,this.skeleton),r.skeleton=this.skeleton.uuid)),this.material!==void 0)if(Array.isArray(this.material)){let a=[];for(let c=0,l=this.material.length;c<l;c++)a.push(s(e.materials,this.material[c]));r.material=a}else r.material=s(e.materials,this.material);if(this.children.length>0){r.children=[];for(let a=0;a<this.children.length;a++)r.children.push(this.children[a].toJSON(e).object)}if(this.animations.length>0){r.animations=[];for(let a=0;a<this.animations.length;a++){let c=this.animations[a];r.animations.push(s(e.animations,c))}}if(t){let a=o(e.geometries),c=o(e.materials),l=o(e.textures),h=o(e.images),m=o(e.shapes),u=o(e.skeletons),g=o(e.animations),_=o(e.nodes);a.length>0&&(i.geometries=a),c.length>0&&(i.materials=c),l.length>0&&(i.textures=l),h.length>0&&(i.images=h),m.length>0&&(i.shapes=m),u.length>0&&(i.skeletons=u),g.length>0&&(i.animations=g),_.length>0&&(i.nodes=_)}return i.object=r,i;function o(a){let c=[];for(let l in a){let h=a[l];delete h.metadata,c.push(h)}return c}}clone(e){return new this.constructor().copy(this,e)}copy(e,t=!0){if(this.name=e.name,this.up.copy(e.up),this.position.copy(e.position),this.rotation.order=e.rotation.order,this.quaternion.copy(e.quaternion),this.scale.copy(e.scale),this.pivot=e.pivot!==null?e.pivot.clone():null,this.matrix.copy(e.matrix),this.matrixWorld.copy(e.matrixWorld),this.matrixAutoUpdate=e.matrixAutoUpdate,this.matrixWorldAutoUpdate=e.matrixWorldAutoUpdate,this.matrixWorldNeedsUpdate=e.matrixWorldNeedsUpdate,this.layers.mask=e.layers.mask,this.visible=e.visible,this.castShadow=e.castShadow,this.receiveShadow=e.receiveShadow,this.frustumCulled=e.frustumCulled,this.renderOrder=e.renderOrder,this.static=e.static,this.animations=e.animations.slice(),this.userData=JSON.parse(JSON.stringify(e.userData)),t===!0)for(let i=0;i<e.children.length;i++){let r=e.children[i];this.add(r.clone())}return this}dispose(){this.dispatchEvent({type:"dispose"})}};dn.DEFAULT_UP=new V(0,1,0);dn.DEFAULT_MATRIX_AUTO_UPDATE=!0;dn.DEFAULT_MATRIX_WORLD_AUTO_UPDATE=!0;var _t=class extends dn{constructor(){super(),this.isGroup=!0,this.type="Group"}},zp={type:"move"},jr=class{constructor(){this._targetRay=null,this._grip=null,this._hand=null}getHandSpace(){return this._hand===null&&(this._hand=new _t,this._hand.matrixAutoUpdate=!1,this._hand.visible=!1,this._hand.joints={},this._hand.inputState={pinching:!1}),this._hand}getTargetRaySpace(){return this._targetRay===null&&(this._targetRay=new _t,this._targetRay.matrixAutoUpdate=!1,this._targetRay.visible=!1,this._targetRay.hasLinearVelocity=!1,this._targetRay.linearVelocity=new V,this._targetRay.hasAngularVelocity=!1,this._targetRay.angularVelocity=new V),this._targetRay}getGripSpace(){return this._grip===null&&(this._grip=new _t,this._grip.matrixAutoUpdate=!1,this._grip.visible=!1,this._grip.hasLinearVelocity=!1,this._grip.linearVelocity=new V,this._grip.hasAngularVelocity=!1,this._grip.angularVelocity=new V,this._grip.eventsEnabled=!1),this._grip}dispatchEvent(e){return this._targetRay!==null&&this._targetRay.dispatchEvent(e),this._grip!==null&&this._grip.dispatchEvent(e),this._hand!==null&&this._hand.dispatchEvent(e),this}connect(e){if(e&&e.hand){let t=this._hand;if(t)for(let i of e.hand.values())this._getHandJoint(t,i)}return this.dispatchEvent({type:"connected",data:e}),this}disconnect(e){return this.dispatchEvent({type:"disconnected",data:e}),this._targetRay!==null&&(this._targetRay.visible=!1),this._grip!==null&&(this._grip.visible=!1),this._hand!==null&&(this._hand.visible=!1),this}update(e,t,i){let r=null,s=null,o=null,a=this._targetRay,c=this._grip,l=this._hand;if(e&&t.session.visibilityState!=="visible-blurred"){if(l&&e.hand){o=!0;for(let x of e.hand.values()){let f=t.getJointPose(x,i),d=this._getHandJoint(l,x);f!==null&&(d.matrix.fromArray(f.transform.matrix),d.matrix.decompose(d.position,d.rotation,d.scale),d.matrixWorldNeedsUpdate=!0,d.jointRadius=f.radius),d.visible=f!==null}let h=l.joints["index-finger-tip"],m=l.joints["thumb-tip"],u=h.position.distanceTo(m.position),g=.02,_=.005;l.inputState.pinching&&u>g+_?(l.inputState.pinching=!1,this.dispatchEvent({type:"pinchend",handedness:e.handedness,target:this})):!l.inputState.pinching&&u<=g-_&&(l.inputState.pinching=!0,this.dispatchEvent({type:"pinchstart",handedness:e.handedness,target:this}))}else c!==null&&e.gripSpace&&(s=t.getPose(e.gripSpace,i),s!==null&&(c.matrix.fromArray(s.transform.matrix),c.matrix.decompose(c.position,c.rotation,c.scale),c.matrixWorldNeedsUpdate=!0,s.linearVelocity?(c.hasLinearVelocity=!0,c.linearVelocity.copy(s.linearVelocity)):c.hasLinearVelocity=!1,s.angularVelocity?(c.hasAngularVelocity=!0,c.angularVelocity.copy(s.angularVelocity)):c.hasAngularVelocity=!1,c.eventsEnabled&&c.dispatchEvent({type:"gripUpdated",data:e,target:this})));a!==null&&(r=t.getPose(e.targetRaySpace,i),r===null&&s!==null&&(r=s),r!==null&&(a.matrix.fromArray(r.transform.matrix),a.matrix.decompose(a.position,a.rotation,a.scale),a.matrixWorldNeedsUpdate=!0,r.linearVelocity?(a.hasLinearVelocity=!0,a.linearVelocity.copy(r.linearVelocity)):a.hasLinearVelocity=!1,r.angularVelocity?(a.hasAngularVelocity=!0,a.angularVelocity.copy(r.angularVelocity)):a.hasAngularVelocity=!1,this.dispatchEvent(zp)))}return a!==null&&(a.visible=r!==null),c!==null&&(c.visible=s!==null),l!==null&&(l.visible=o!==null),this}_getHandJoint(e,t){if(e.joints[t.jointName]===void 0){let i=new _t;i.matrixAutoUpdate=!1,i.visible=!1,e.joints[t.jointName]=i,e.add(i)}return e.joints[t.jointName]}},md={aliceblue:15792383,antiquewhite:16444375,aqua:65535,aquamarine:8388564,azure:15794175,beige:16119260,bisque:16770244,black:0,blanchedalmond:16772045,blue:255,blueviolet:9055202,brown:10824234,burlywood:14596231,cadetblue:6266528,chartreuse:8388352,chocolate:13789470,coral:16744272,cornflowerblue:6591981,cornsilk:16775388,crimson:14423100,cyan:65535,darkblue:139,darkcyan:35723,darkgoldenrod:12092939,darkgray:11119017,darkgreen:25600,darkgrey:11119017,darkkhaki:12433259,darkmagenta:9109643,darkolivegreen:5597999,darkorange:16747520,darkorchid:10040012,darkred:9109504,darksalmon:15308410,darkseagreen:9419919,darkslateblue:4734347,darkslategray:3100495,darkslategrey:3100495,darkturquoise:52945,darkviolet:9699539,deeppink:16716947,deepskyblue:49151,dimgray:6908265,dimgrey:6908265,dodgerblue:2003199,firebrick:11674146,floralwhite:16775920,forestgreen:2263842,fuchsia:16711935,gainsboro:14474460,ghostwhite:16316671,gold:16766720,goldenrod:14329120,gray:8421504,green:32768,greenyellow:11403055,grey:8421504,honeydew:15794160,hotpink:16738740,indianred:13458524,indigo:4915330,ivory:16777200,khaki:15787660,lavender:15132410,lavenderblush:16773365,lawngreen:8190976,lemonchiffon:16775885,lightblue:11393254,lightcoral:15761536,lightcyan:14745599,lightgoldenrodyellow:16448210,lightgray:13882323,lightgreen:9498256,lightgrey:13882323,lightpink:16758465,lightsalmon:16752762,lightseagreen:2142890,lightskyblue:8900346,lightslategray:7833753,lightslategrey:7833753,lightsteelblue:11584734,lightyellow:16777184,lime:65280,limegreen:3329330,linen:16445670,magenta:16711935,maroon:8388608,mediumaquamarine:6737322,mediumblue:205,mediumorchid:12211667,mediumpurple:9662683,mediumseagreen:3978097,mediumslateblue:8087790,mediumspringgreen:64154,mediumturquoise:4772300,mediumvioletred:13047173,midnightblue:1644912,mintcream:16121850,mistyrose:16770273,moccasin:16770229,navajowhite:16768685,navy:128,oldlace:16643558,olive:8421376,olivedrab:7048739,orange:16753920,orangered:16729344,orchid:14315734,palegoldenrod:15657130,palegreen:10025880,paleturquoise:11529966,palevioletred:14381203,papayawhip:16773077,peachpuff:16767673,peru:13468991,pink:16761035,plum:14524637,powderblue:11591910,purple:8388736,rebeccapurple:6697881,red:16711680,rosybrown:12357519,royalblue:4286945,saddlebrown:9127187,salmon:16416882,sandybrown:16032864,seagreen:3050327,seashell:16774638,sienna:10506797,silver:12632256,skyblue:8900331,slateblue:6970061,slategray:7372944,slategrey:7372944,snow:16775930,springgreen:65407,steelblue:4620980,tan:13808780,teal:32896,thistle:14204888,tomato:16737095,turquoise:4251856,violet:15631086,wheat:16113331,white:16777215,whitesmoke:16119285,yellow:16776960,yellowgreen:10145074},Fi={h:0,s:0,l:0},Eo={h:0,s:0,l:0};function Ql(n,e,t){return t<0&&(t+=1),t>1&&(t-=1),t<1/6?n+(e-n)*6*t:t<1/2?e:t<2/3?n+(e-n)*6*(2/3-t):n}var ze=class{constructor(e,t,i){return this.isColor=!0,this.r=1,this.g=1,this.b=1,this.set(e,t,i)}set(e,t,i){if(t===void 0&&i===void 0){let r=e;r&&r.isColor?this.copy(r):typeof r=="number"?this.setHex(r):typeof r=="string"&&this.setStyle(r)}else this.setRGB(e,t,i);return this}setScalar(e){return this.r=e,this.g=e,this.b=e,this}setHex(e,t=Ht){return e=Math.floor(e),this.r=(e>>16&255)/255,this.g=(e>>8&255)/255,this.b=(e&255)/255,lt.colorSpaceToWorking(this,t),this}setRGB(e,t,i,r=lt.workingColorSpace){return this.r=e,this.g=t,this.b=i,lt.colorSpaceToWorking(this,r),this}setHSL(e,t,i,r=lt.workingColorSpace){if(e=Kc(e,1),t=it(t,0,1),i=it(i,0,1),t===0)this.r=this.g=this.b=i;else{let s=i<=.5?i*(1+t):i+t-i*t,o=2*i-s;this.r=Ql(o,s,e+1/3),this.g=Ql(o,s,e),this.b=Ql(o,s,e-1/3)}return lt.colorSpaceToWorking(this,r),this}setStyle(e,t=Ht){function i(s){s!==void 0&&parseFloat(s)<1&&Ge("Color: Alpha component of "+e+" will be ignored.")}let r;if(r=/^(\w+)\(([^\)]*)\)/.exec(e)){let s,o=r[1],a=r[2];switch(o){case"rgb":case"rgba":if(s=/^\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*(\d*\.?\d+)\s*)?$/.exec(a))return i(s[4]),this.setRGB(Math.min(255,parseInt(s[1],10))/255,Math.min(255,parseInt(s[2],10))/255,Math.min(255,parseInt(s[3],10))/255,t);if(s=/^\s*(\d+)\%\s*,\s*(\d+)\%\s*,\s*(\d+)\%\s*(?:,\s*(\d*\.?\d+)\s*)?$/.exec(a))return i(s[4]),this.setRGB(Math.min(100,parseInt(s[1],10))/100,Math.min(100,parseInt(s[2],10))/100,Math.min(100,parseInt(s[3],10))/100,t);break;case"hsl":case"hsla":if(s=/^\s*(\d*\.?\d+)\s*,\s*(\d*\.?\d+)\%\s*,\s*(\d*\.?\d+)\%\s*(?:,\s*(\d*\.?\d+)\s*)?$/.exec(a))return i(s[4]),this.setHSL(parseFloat(s[1])/360,parseFloat(s[2])/100,parseFloat(s[3])/100,t);break;default:Ge("Color: Unknown color model "+e)}}else if(r=/^\#([A-Fa-f\d]+)$/.exec(e)){let s=r[1],o=s.length;if(o===3)return this.setRGB(parseInt(s.charAt(0),16)/15,parseInt(s.charAt(1),16)/15,parseInt(s.charAt(2),16)/15,t);if(o===6)return this.setHex(parseInt(s,16),t);Ge("Color: Invalid hex color "+e)}else if(e&&e.length>0)return this.setColorName(e,t);return this}setColorName(e,t=Ht){let i=md[e.toLowerCase()];return i!==void 0?this.setHex(i,t):Ge("Color: Unknown color "+e),this}clone(){return new this.constructor(this.r,this.g,this.b)}copy(e){return this.r=e.r,this.g=e.g,this.b=e.b,this}copySRGBToLinear(e){return this.r=Ti(e.r),this.g=Ti(e.g),this.b=Ti(e.b),this}copyLinearToSRGB(e){return this.r=Xr(e.r),this.g=Xr(e.g),this.b=Xr(e.b),this}convertSRGBToLinear(){return this.copySRGBToLinear(this),this}convertLinearToSRGB(){return this.copyLinearToSRGB(this),this}getHex(e=Ht){return lt.workingToColorSpace(Mn.copy(this),e),Math.round(it(Mn.r*255,0,255))*65536+Math.round(it(Mn.g*255,0,255))*256+Math.round(it(Mn.b*255,0,255))}getHexString(e=Ht){return("000000"+this.getHex(e).toString(16)).slice(-6)}getHSL(e,t=lt.workingColorSpace){lt.workingToColorSpace(Mn.copy(this),t);let i=Mn.r,r=Mn.g,s=Mn.b,o=Math.max(i,r,s),a=Math.min(i,r,s),c,l,h=(a+o)/2;if(a===o)c=0,l=0;else{let m=o-a;switch(l=h<=.5?m/(o+a):m/(2-o-a),o){case i:c=(r-s)/m+(r<s?6:0);break;case r:c=(s-i)/m+2;break;case s:c=(i-r)/m+4;break}c/=6}return e.h=c,e.s=l,e.l=h,e}getRGB(e,t=lt.workingColorSpace){return lt.workingToColorSpace(Mn.copy(this),t),e.r=Mn.r,e.g=Mn.g,e.b=Mn.b,e}getStyle(e=Ht){lt.workingToColorSpace(Mn.copy(this),e);let t=Mn.r,i=Mn.g,r=Mn.b;return e!==Ht?`color(${e} ${t.toFixed(3)} ${i.toFixed(3)} ${r.toFixed(3)})`:`rgb(${Math.round(t*255)},${Math.round(i*255)},${Math.round(r*255)})`}offsetHSL(e,t,i){return this.getHSL(Fi),this.setHSL(Fi.h+e,Fi.s+t,Fi.l+i)}add(e){return this.r+=e.r,this.g+=e.g,this.b+=e.b,this}addColors(e,t){return this.r=e.r+t.r,this.g=e.g+t.g,this.b=e.b+t.b,this}addScalar(e){return this.r+=e,this.g+=e,this.b+=e,this}sub(e){return this.r=Math.max(0,this.r-e.r),this.g=Math.max(0,this.g-e.g),this.b=Math.max(0,this.b-e.b),this}multiply(e){return this.r*=e.r,this.g*=e.g,this.b*=e.b,this}multiplyScalar(e){return this.r*=e,this.g*=e,this.b*=e,this}lerp(e,t){return this.r+=(e.r-this.r)*t,this.g+=(e.g-this.g)*t,this.b+=(e.b-this.b)*t,this}lerpColors(e,t,i){return this.r=e.r+(t.r-e.r)*i,this.g=e.g+(t.g-e.g)*i,this.b=e.b+(t.b-e.b)*i,this}lerpHSL(e,t){this.getHSL(Fi),e.getHSL(Eo);let i=Ps(Fi.h,Eo.h,t),r=Ps(Fi.s,Eo.s,t),s=Ps(Fi.l,Eo.l,t);return this.setHSL(i,r,s),this}setFromVector3(e){return this.r=e.x,this.g=e.y,this.b=e.z,this}applyMatrix3(e){let t=this.r,i=this.g,r=this.b,s=e.elements;return this.r=s[0]*t+s[3]*i+s[6]*r,this.g=s[1]*t+s[4]*i+s[7]*r,this.b=s[2]*t+s[5]*i+s[8]*r,this}equals(e){return e.r===this.r&&e.g===this.g&&e.b===this.b}fromArray(e,t=0){return this.r=e[t],this.g=e[t+1],this.b=e[t+2],this}toArray(e=[],t=0){return e[t]=this.r,e[t+1]=this.g,e[t+2]=this.b,e}fromBufferAttribute(e,t){return this.r=e.getX(t),this.g=e.getY(t),this.b=e.getZ(t),this}toJSON(){return this.getHex()}*[Symbol.iterator](){yield this.r,yield this.g,yield this.b}},Mn=new ze;ze.NAMES=md;var Us=class extends dn{constructor(){super(),this.isScene=!0,this.type="Scene",this.background=null,this.environment=null,this.fog=null,this.backgroundBlurriness=0,this.backgroundIntensity=1,this.backgroundRotation=new Ai,this.environmentIntensity=1,this.environmentRotation=new Ai,this.overrideMaterial=null,typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("observe",{detail:this}))}copy(e,t){return super.copy(e,t),e.background!==null&&(this.background=e.background.clone()),e.environment!==null&&(this.environment=e.environment.clone()),e.fog!==null&&(this.fog=e.fog.clone()),this.backgroundBlurriness=e.backgroundBlurriness,this.backgroundIntensity=e.backgroundIntensity,this.backgroundRotation.copy(e.backgroundRotation),this.environmentIntensity=e.environmentIntensity,this.environmentRotation.copy(e.environmentRotation),e.overrideMaterial!==null&&(this.overrideMaterial=e.overrideMaterial.clone()),this.matrixAutoUpdate=e.matrixAutoUpdate,this}toJSON(e){let t=super.toJSON(e);return this.fog!==null&&(t.object.fog=this.fog.toJSON()),t.object.backgroundBlurriness=this.backgroundBlurriness,t.object.backgroundIntensity=this.backgroundIntensity,t.object.backgroundRotation=this.backgroundRotation.toArray(),t.object.environmentIntensity=this.environmentIntensity,t.object.environmentRotation=this.environmentRotation.toArray(),t}},jn=new V,bi=new V,ec=new V,Mi=new V,Dr=new V,Nr=new V,ph=new V,tc=new V,nc=new V,ic=new V,rc=new Ot,sc=new Ot,oc=new Ot,Ei=class n{constructor(e=new V,t=new V,i=new V){this.a=e,this.b=t,this.c=i}static getNormal(e,t,i,r){r.subVectors(i,t),jn.subVectors(e,t),r.cross(jn);let s=r.lengthSq();return s>0?r.multiplyScalar(1/Math.sqrt(s)):r.set(0,0,0)}static getBarycoord(e,t,i,r,s){jn.subVectors(r,t),bi.subVectors(i,t),ec.subVectors(e,t);let o=jn.dot(jn),a=jn.dot(bi),c=jn.dot(ec),l=bi.dot(bi),h=bi.dot(ec),m=o*l-a*a;if(m===0)return s.set(0,0,0),null;let u=1/m,g=(l*c-a*h)*u,_=(o*h-a*c)*u;return s.set(1-g-_,_,g)}static containsPoint(e,t,i,r){return this.getBarycoord(e,t,i,r,Mi)===null?!1:Mi.x>=0&&Mi.y>=0&&Mi.x+Mi.y<=1}static getInterpolation(e,t,i,r,s,o,a,c){return this.getBarycoord(e,t,i,r,Mi)===null?(c.x=0,c.y=0,"z"in c&&(c.z=0),"w"in c&&(c.w=0),null):(c.setScalar(0),c.addScaledVector(s,Mi.x),c.addScaledVector(o,Mi.y),c.addScaledVector(a,Mi.z),c)}static getInterpolatedAttribute(e,t,i,r,s,o){return rc.setScalar(0),sc.setScalar(0),oc.setScalar(0),rc.fromBufferAttribute(e,t),sc.fromBufferAttribute(e,i),oc.fromBufferAttribute(e,r),o.setScalar(0),o.addScaledVector(rc,s.x),o.addScaledVector(sc,s.y),o.addScaledVector(oc,s.z),o}static isFrontFacing(e,t,i,r){return jn.subVectors(i,t),bi.subVectors(e,t),jn.cross(bi).dot(r)<0}set(e,t,i){return this.a.copy(e),this.b.copy(t),this.c.copy(i),this}setFromPointsAndIndices(e,t,i,r){return this.a.copy(e[t]),this.b.copy(e[i]),this.c.copy(e[r]),this}setFromAttributeAndIndices(e,t,i,r){return this.a.fromBufferAttribute(e,t),this.b.fromBufferAttribute(e,i),this.c.fromBufferAttribute(e,r),this}clone(){return new this.constructor().copy(this)}copy(e){return this.a.copy(e.a),this.b.copy(e.b),this.c.copy(e.c),this}getArea(){return jn.subVectors(this.c,this.b),bi.subVectors(this.a,this.b),jn.cross(bi).length()*.5}getMidpoint(e){return e.addVectors(this.a,this.b).add(this.c).multiplyScalar(1/3)}getNormal(e){return n.getNormal(this.a,this.b,this.c,e)}getPlane(e){return e.setFromCoplanarPoints(this.a,this.b,this.c)}getBarycoord(e,t){return n.getBarycoord(e,this.a,this.b,this.c,t)}getInterpolation(e,t,i,r,s){return n.getInterpolation(e,this.a,this.b,this.c,t,i,r,s)}containsPoint(e){return n.containsPoint(e,this.a,this.b,this.c)}isFrontFacing(e){return n.isFrontFacing(this.a,this.b,this.c,e)}intersectsBox(e){return e.intersectsTriangle(this)}closestPointToPoint(e,t){let i=this.a,r=this.b,s=this.c,o,a;Dr.subVectors(r,i),Nr.subVectors(s,i),tc.subVectors(e,i);let c=Dr.dot(tc),l=Nr.dot(tc);if(c<=0&&l<=0)return t.copy(i);nc.subVectors(e,r);let h=Dr.dot(nc),m=Nr.dot(nc);if(h>=0&&m<=h)return t.copy(r);let u=c*m-h*l;if(u<=0&&c>=0&&h<=0)return o=c/(c-h),t.copy(i).addScaledVector(Dr,o);ic.subVectors(e,s);let g=Dr.dot(ic),_=Nr.dot(ic);if(_>=0&&g<=_)return t.copy(s);let x=g*l-c*_;if(x<=0&&l>=0&&_<=0)return a=l/(l-_),t.copy(i).addScaledVector(Nr,a);let f=h*_-g*m;if(f<=0&&m-h>=0&&g-_>=0)return ph.subVectors(s,r),a=(m-h)/(m-h+(g-_)),t.copy(r).addScaledVector(ph,a);let d=1/(f+x+u);return o=x*d,a=u*d,t.copy(i).addScaledVector(Dr,o).addScaledVector(Nr,a)}equals(e){return e.a.equals(this.a)&&e.b.equals(this.b)&&e.c.equals(this.c)}},Xn=class{constructor(e=new V(1/0,1/0,1/0),t=new V(-1/0,-1/0,-1/0)){this.isBox3=!0,this.min=e,this.max=t}set(e,t){return this.min.copy(e),this.max.copy(t),this}setFromArray(e){this.makeEmpty();for(let t=0,i=e.length;t<i;t+=3)this.expandByPoint(Qn.fromArray(e,t));return this}setFromBufferAttribute(e){this.makeEmpty();for(let t=0,i=e.count;t<i;t++)this.expandByPoint(Qn.fromBufferAttribute(e,t));return this}setFromPoints(e){this.makeEmpty();for(let t=0,i=e.length;t<i;t++)this.expandByPoint(e[t]);return this}setFromCenterAndSize(e,t){let i=Qn.copy(t).multiplyScalar(.5);return this.min.copy(e).sub(i),this.max.copy(e).add(i),this}setFromObject(e,t=!1){return this.makeEmpty(),this.expandByObject(e,t)}clone(){return new this.constructor().copy(this)}copy(e){return this.min.copy(e.min),this.max.copy(e.max),this}makeEmpty(){return this.min.x=this.min.y=this.min.z=1/0,this.max.x=this.max.y=this.max.z=-1/0,this}isEmpty(){return this.max.x<this.min.x||this.max.y<this.min.y||this.max.z<this.min.z}getCenter(e){return this.isEmpty()?e.set(0,0,0):e.addVectors(this.min,this.max).multiplyScalar(.5)}getSize(e){return this.isEmpty()?e.set(0,0,0):e.subVectors(this.max,this.min)}expandByPoint(e){return this.min.min(e),this.max.max(e),this}expandByVector(e){return this.min.sub(e),this.max.add(e),this}expandByScalar(e){return this.min.addScalar(-e),this.max.addScalar(e),this}expandByObject(e,t=!1){e.updateWorldMatrix(!1,!1);let i=e.geometry;if(i!==void 0){let s=i.getAttribute("position");if(t===!0&&s!==void 0&&e.isInstancedMesh!==!0)for(let o=0,a=s.count;o<a;o++)e.isMesh===!0?e.getVertexPosition(o,Qn):Qn.fromBufferAttribute(s,o),Qn.applyMatrix4(e.matrixWorld),this.expandByPoint(Qn);else e.boundingBox!==void 0?(e.boundingBox===null&&e.computeBoundingBox(),To.copy(e.boundingBox)):(i.boundingBox===null&&i.computeBoundingBox(),To.copy(i.boundingBox)),To.applyMatrix4(e.matrixWorld),this.union(To)}let r=e.children;for(let s=0,o=r.length;s<o;s++)this.expandByObject(r[s],t);return this}containsPoint(e){return e.x>=this.min.x&&e.x<=this.max.x&&e.y>=this.min.y&&e.y<=this.max.y&&e.z>=this.min.z&&e.z<=this.max.z}containsBox(e){return this.min.x<=e.min.x&&e.max.x<=this.max.x&&this.min.y<=e.min.y&&e.max.y<=this.max.y&&this.min.z<=e.min.z&&e.max.z<=this.max.z}getParameter(e,t){return t.set((e.x-this.min.x)/(this.max.x-this.min.x),(e.y-this.min.y)/(this.max.y-this.min.y),(e.z-this.min.z)/(this.max.z-this.min.z))}intersectsBox(e){return e.max.x>=this.min.x&&e.min.x<=this.max.x&&e.max.y>=this.min.y&&e.min.y<=this.max.y&&e.max.z>=this.min.z&&e.min.z<=this.max.z}intersectsSphere(e){return this.clampPoint(e.center,Qn),Qn.distanceToSquared(e.center)<=e.radius*e.radius}intersectsPlane(e){let t,i;return e.normal.x>0?(t=e.normal.x*this.min.x,i=e.normal.x*this.max.x):(t=e.normal.x*this.max.x,i=e.normal.x*this.min.x),e.normal.y>0?(t+=e.normal.y*this.min.y,i+=e.normal.y*this.max.y):(t+=e.normal.y*this.max.y,i+=e.normal.y*this.min.y),e.normal.z>0?(t+=e.normal.z*this.min.z,i+=e.normal.z*this.max.z):(t+=e.normal.z*this.max.z,i+=e.normal.z*this.min.z),t<=-e.constant&&i>=-e.constant}intersectsTriangle(e){if(this.isEmpty())return!1;this.getCenter(Ts),Ao.subVectors(this.max,Ts),Ur.subVectors(e.a,Ts),Fr.subVectors(e.b,Ts),Or.subVectors(e.c,Ts),Oi.subVectors(Fr,Ur),Bi.subVectors(Or,Fr),sr.subVectors(Ur,Or);let t=[0,-Oi.z,Oi.y,0,-Bi.z,Bi.y,0,-sr.z,sr.y,Oi.z,0,-Oi.x,Bi.z,0,-Bi.x,sr.z,0,-sr.x,-Oi.y,Oi.x,0,-Bi.y,Bi.x,0,-sr.y,sr.x,0];return!ac(t,Ur,Fr,Or,Ao)||(t=[1,0,0,0,1,0,0,0,1],!ac(t,Ur,Fr,Or,Ao))?!1:(Ro.crossVectors(Oi,Bi),t=[Ro.x,Ro.y,Ro.z],ac(t,Ur,Fr,Or,Ao))}clampPoint(e,t){return t.copy(e).clamp(this.min,this.max)}distanceToPoint(e){return this.clampPoint(e,Qn).distanceTo(e)}getBoundingSphere(e){return this.isEmpty()?e.makeEmpty():(this.getCenter(e.center),e.radius=this.getSize(Qn).length()*.5),e}intersect(e){return this.min.max(e.min),this.max.min(e.max),this.isEmpty()&&this.makeEmpty(),this}union(e){return this.min.min(e.min),this.max.max(e.max),this}applyMatrix4(e){return this.isEmpty()?this:(Si[0].set(this.min.x,this.min.y,this.min.z).applyMatrix4(e),Si[1].set(this.min.x,this.min.y,this.max.z).applyMatrix4(e),Si[2].set(this.min.x,this.max.y,this.min.z).applyMatrix4(e),Si[3].set(this.min.x,this.max.y,this.max.z).applyMatrix4(e),Si[4].set(this.max.x,this.min.y,this.min.z).applyMatrix4(e),Si[5].set(this.max.x,this.min.y,this.max.z).applyMatrix4(e),Si[6].set(this.max.x,this.max.y,this.min.z).applyMatrix4(e),Si[7].set(this.max.x,this.max.y,this.max.z).applyMatrix4(e),this.setFromPoints(Si),this)}translate(e){return this.min.add(e),this.max.add(e),this}equals(e){return e.min.equals(this.min)&&e.max.equals(this.max)}toJSON(){return{min:this.min.toArray(),max:this.max.toArray()}}fromJSON(e){return this.min.fromArray(e.min),this.max.fromArray(e.max),this}},Si=[new V,new V,new V,new V,new V,new V,new V,new V],Qn=new V,To=new Xn,Ur=new V,Fr=new V,Or=new V,Oi=new V,Bi=new V,sr=new V,Ts=new V,Ao=new V,Ro=new V,or=new V;function ac(n,e,t,i,r){for(let s=0,o=n.length-3;s<=o;s+=3){or.fromArray(n,s);let a=r.x*Math.abs(or.x)+r.y*Math.abs(or.y)+r.z*Math.abs(or.z),c=e.dot(or),l=t.dot(or),h=i.dot(or);if(Math.max(-Math.max(c,l,h),Math.min(c,l,h))>a)return!1}return!0}var rn=new V,Co=new He,Hp=0,Pn=class extends ti{constructor(e,t,i=!1){if(super(),Array.isArray(e))throw new TypeError("THREE.BufferAttribute: array should be a Typed Array.");this.isBufferAttribute=!0,Object.defineProperty(this,"id",{value:Hp++}),this.name="",this.array=e,this.itemSize=t,this.count=e!==void 0?e.length/t:0,this.normalized=i,this.usage=ud,this.updateRanges=[],this.gpuType=ri,this.version=0}onUploadCallback(){}set needsUpdate(e){e===!0&&this.version++}setUsage(e){return this.usage=e,this}addUpdateRange(e,t){this.updateRanges.push({start:e,count:t})}clearUpdateRanges(){this.updateRanges.length=0}copy(e){return this.name=e.name,this.array=new e.array.constructor(e.array),this.itemSize=e.itemSize,this.count=e.count,this.normalized=e.normalized,this.usage=e.usage,this.gpuType=e.gpuType,this}copyAt(e,t,i){e*=this.itemSize,i*=t.itemSize;for(let r=0,s=this.itemSize;r<s;r++)this.array[e+r]=t.array[i+r];return this}copyArray(e){return this.array.set(e),this}applyMatrix3(e){if(this.itemSize===2)for(let t=0,i=this.count;t<i;t++)Co.fromBufferAttribute(this,t),Co.applyMatrix3(e),this.setXY(t,Co.x,Co.y);else if(this.itemSize===3)for(let t=0,i=this.count;t<i;t++)rn.fromBufferAttribute(this,t),rn.applyMatrix3(e),this.setXYZ(t,rn.x,rn.y,rn.z);return this}applyMatrix4(e){for(let t=0,i=this.count;t<i;t++)rn.fromBufferAttribute(this,t),rn.applyMatrix4(e),this.setXYZ(t,rn.x,rn.y,rn.z);return this}applyNormalMatrix(e){for(let t=0,i=this.count;t<i;t++)rn.fromBufferAttribute(this,t),rn.applyNormalMatrix(e),this.setXYZ(t,rn.x,rn.y,rn.z);return this}transformDirection(e){for(let t=0,i=this.count;t<i;t++)rn.fromBufferAttribute(this,t),rn.transformDirection(e),this.setXYZ(t,rn.x,rn.y,rn.z);return this}set(e,t=0){return this.array.set(e,t),this}getComponent(e,t){let i=this.array[e*this.itemSize+t];return this.normalized&&(i=Gr(i,this.array)),i}setComponent(e,t,i){return this.normalized&&(i=Tn(i,this.array)),this.array[e*this.itemSize+t]=i,this}getX(e){let t=this.array[e*this.itemSize];return this.normalized&&(t=Gr(t,this.array)),t}setX(e,t){return this.normalized&&(t=Tn(t,this.array)),this.array[e*this.itemSize]=t,this}getY(e){let t=this.array[e*this.itemSize+1];return this.normalized&&(t=Gr(t,this.array)),t}setY(e,t){return this.normalized&&(t=Tn(t,this.array)),this.array[e*this.itemSize+1]=t,this}getZ(e){let t=this.array[e*this.itemSize+2];return this.normalized&&(t=Gr(t,this.array)),t}setZ(e,t){return this.normalized&&(t=Tn(t,this.array)),this.array[e*this.itemSize+2]=t,this}getW(e){let t=this.array[e*this.itemSize+3];return this.normalized&&(t=Gr(t,this.array)),t}setW(e,t){return this.normalized&&(t=Tn(t,this.array)),this.array[e*this.itemSize+3]=t,this}setXY(e,t,i){return e*=this.itemSize,this.normalized&&(t=Tn(t,this.array),i=Tn(i,this.array)),this.array[e+0]=t,this.array[e+1]=i,this}setXYZ(e,t,i,r){return e*=this.itemSize,this.normalized&&(t=Tn(t,this.array),i=Tn(i,this.array),r=Tn(r,this.array)),this.array[e+0]=t,this.array[e+1]=i,this.array[e+2]=r,this}setXYZW(e,t,i,r,s){return e*=this.itemSize,this.normalized&&(t=Tn(t,this.array),i=Tn(i,this.array),r=Tn(r,this.array),s=Tn(s,this.array)),this.array[e+0]=t,this.array[e+1]=i,this.array[e+2]=r,this.array[e+3]=s,this}onUpload(e){return this.onUploadCallback=e,this}clone(){return new this.constructor(this.array,this.itemSize).copy(this)}toJSON(){let e={itemSize:this.itemSize,type:this.array.constructor.name,array:Array.from(this.array),normalized:this.normalized};return e.name=this.name,e.usage=this.usage,e.gpuType=this.gpuType,e}dispose(){this.dispatchEvent({type:"dispose"})}};var Fs=class extends Pn{constructor(e,t,i){super(new Uint16Array(e),t,i)}};var Os=class extends Pn{constructor(e,t,i){super(new Uint32Array(e),t,i)}};var ct=class extends Pn{constructor(e,t,i){super(new Float32Array(e),t,i)}},Vp=new Xn,As=new V,lc=new V,Hi=class{constructor(e=new V,t=-1){this.isSphere=!0,this.center=e,this.radius=t}set(e,t){return this.center.copy(e),this.radius=t,this}setFromPoints(e,t){let i=this.center;t!==void 0?i.copy(t):Vp.setFromPoints(e).getCenter(i);let r=0;for(let s=0,o=e.length;s<o;s++)r=Math.max(r,i.distanceToSquared(e[s]));return this.radius=Math.sqrt(r),this}copy(e){return this.center.copy(e.center),this.radius=e.radius,this}isEmpty(){return this.radius<0}makeEmpty(){return this.center.set(0,0,0),this.radius=-1,this}containsPoint(e){return e.distanceToSquared(this.center)<=this.radius*this.radius}distanceToPoint(e){return e.distanceTo(this.center)-this.radius}intersectsSphere(e){let t=this.radius+e.radius;return e.center.distanceToSquared(this.center)<=t*t}intersectsBox(e){return e.intersectsSphere(this)}intersectsPlane(e){return Math.abs(e.distanceToPoint(this.center))<=this.radius}clampPoint(e,t){let i=this.center.distanceToSquared(e);return t.copy(e),i>this.radius*this.radius&&(t.sub(this.center).normalize(),t.multiplyScalar(this.radius).add(this.center)),t}getBoundingBox(e){return this.isEmpty()?(e.makeEmpty(),e):(e.set(this.center,this.center),e.expandByScalar(this.radius),e)}applyMatrix4(e){return this.center.applyMatrix4(e),this.radius=this.radius*e.getMaxScaleOnAxis(),this}translate(e){return this.center.add(e),this}expandByPoint(e){if(this.isEmpty())return this.center.copy(e),this.radius=0,this;As.subVectors(e,this.center);let t=As.lengthSq();if(t>this.radius*this.radius){let i=Math.sqrt(t),r=(i-this.radius)*.5;this.center.addScaledVector(As,r/i),this.radius+=r}return this}union(e){return e.isEmpty()?this:this.isEmpty()?(this.copy(e),this):(this.center.equals(e.center)===!0?this.radius=Math.max(this.radius,e.radius):(lc.subVectors(e.center,this.center).setLength(e.radius),this.expandByPoint(As.copy(e.center).add(lc)),this.expandByPoint(As.copy(e.center).sub(lc))),this)}equals(e){return e.center.equals(this.center)&&e.radius===this.radius}clone(){return new this.constructor().copy(this)}toJSON(){return{radius:this.radius,center:this.center.toArray()}}fromJSON(e){return this.radius=e.radius,this.center.fromArray(e.center),this}},Gp=0,Wn=new It,cc=new dn,Br=new V,Nn=new Xn,Rs=new Xn,un=new V,Xt=class n extends ti{constructor(){super(),this.isBufferGeometry=!0,Object.defineProperty(this,"id",{value:Gp++}),this.uuid=hs(),this.name="",this.type="BufferGeometry",this.index=null,this.indirect=null,this.indirectOffset=0,this.attributes={},this.morphAttributes={},this.morphTargetsRelative=!1,this.groups=[],this.boundingBox=null,this.boundingSphere=null,this.drawRange={start:0,count:1/0},this.userData={},this._transformed=!1}getIndex(){return this.index}setIndex(e){return Array.isArray(e)?this.index=new(fp(e)?Os:Fs)(e,1):this.index=e,this}setIndirect(e,t=0){return this.indirect=e,this.indirectOffset=t,this}getIndirect(){return this.indirect}getAttribute(e){return this.attributes[e]}setAttribute(e,t){return this.attributes[e]=t,this}deleteAttribute(e){return delete this.attributes[e],this}hasAttribute(e){return this.attributes[e]!==void 0}addGroup(e,t,i=0){this.groups.push({start:e,count:t,materialIndex:i})}clearGroups(){this.groups=[]}setDrawRange(e,t){this.drawRange.start=e,this.drawRange.count=t}applyMatrix4(e){let t=this.attributes.position;t!==void 0&&(t.applyMatrix4(e),t.needsUpdate=!0);let i=this.attributes.normal;if(i!==void 0){let s=new Ze().getNormalMatrix(e);i.applyNormalMatrix(s),i.needsUpdate=!0}let r=this.attributes.tangent;return r!==void 0&&(r.transformDirection(e),r.needsUpdate=!0),this.boundingBox!==null&&this.computeBoundingBox(),this.boundingSphere!==null&&this.computeBoundingSphere(),this._transformed=!0,this}applyQuaternion(e){return Wn.makeRotationFromQuaternion(e),this.applyMatrix4(Wn),this}rotateX(e){return Wn.makeRotationX(e),this.applyMatrix4(Wn),this}rotateY(e){return Wn.makeRotationY(e),this.applyMatrix4(Wn),this}rotateZ(e){return Wn.makeRotationZ(e),this.applyMatrix4(Wn),this}translate(e,t,i){return Wn.makeTranslation(e,t,i),this.applyMatrix4(Wn),this}scale(e,t,i){return Wn.makeScale(e,t,i),this.applyMatrix4(Wn),this}lookAt(e){return cc.lookAt(e),cc.updateMatrix(),this.applyMatrix4(cc.matrix),this}center(){return this.computeBoundingBox(),this.boundingBox.getCenter(Br).negate(),this.translate(Br.x,Br.y,Br.z),this}setFromPoints(e){let t=this.getAttribute("position");if(t===void 0){let i=[];for(let r=0,s=e.length;r<s;r++){let o=e[r];i.push(o.x,o.y,o.z||0)}this.setAttribute("position",new ct(i,3))}else{let i=Math.min(e.length,t.count);for(let r=0;r<i;r++){let s=e[r];t.setXYZ(r,s.x,s.y,s.z||0)}e.length>t.count&&Ge("BufferGeometry: Buffer size too small for points data. Use .dispose() and create a new geometry."),t.needsUpdate=!0}return this}computeBoundingBox(){this.boundingBox===null&&(this.boundingBox=new Xn);let e=this.attributes.position,t=this.morphAttributes.position;if(e&&e.isGLBufferAttribute){Xe("BufferGeometry.computeBoundingBox(): GLBufferAttribute requires a manual bounding box.",this),this.boundingBox.set(new V(-1/0,-1/0,-1/0),new V(1/0,1/0,1/0));return}if(e!==void 0){if(this.boundingBox.setFromBufferAttribute(e),t)for(let i=0,r=t.length;i<r;i++){let s=t[i];Nn.setFromBufferAttribute(s),this.morphTargetsRelative?(un.addVectors(this.boundingBox.min,Nn.min),this.boundingBox.expandByPoint(un),un.addVectors(this.boundingBox.max,Nn.max),this.boundingBox.expandByPoint(un)):(this.boundingBox.expandByPoint(Nn.min),this.boundingBox.expandByPoint(Nn.max))}}else this.boundingBox.makeEmpty();(isNaN(this.boundingBox.min.x)||isNaN(this.boundingBox.min.y)||isNaN(this.boundingBox.min.z))&&Xe('BufferGeometry.computeBoundingBox(): Computed min/max have NaN values. The "position" attribute is likely to have NaN values.',this)}computeBoundingSphere(){this.boundingSphere===null&&(this.boundingSphere=new Hi);let e=this.attributes.position,t=this.morphAttributes.position;if(e&&e.isGLBufferAttribute){Xe("BufferGeometry.computeBoundingSphere(): GLBufferAttribute requires a manual bounding sphere.",this),this.boundingSphere.set(new V,1/0);return}if(e){let i=this.boundingSphere.center;if(Nn.setFromBufferAttribute(e),t)for(let s=0,o=t.length;s<o;s++){let a=t[s];Rs.setFromBufferAttribute(a),this.morphTargetsRelative?(un.addVectors(Nn.min,Rs.min),Nn.expandByPoint(un),un.addVectors(Nn.max,Rs.max),Nn.expandByPoint(un)):(Nn.expandByPoint(Rs.min),Nn.expandByPoint(Rs.max))}Nn.getCenter(i);let r=0;for(let s=0,o=e.count;s<o;s++)un.fromBufferAttribute(e,s),r=Math.max(r,i.distanceToSquared(un));if(t)for(let s=0,o=t.length;s<o;s++){let a=t[s],c=this.morphTargetsRelative;for(let l=0,h=a.count;l<h;l++)un.fromBufferAttribute(a,l),c&&(Br.fromBufferAttribute(e,l),un.add(Br)),r=Math.max(r,i.distanceToSquared(un))}this.boundingSphere.radius=Math.sqrt(r),isNaN(this.boundingSphere.radius)&&Xe('BufferGeometry.computeBoundingSphere(): Computed radius is NaN. The "position" attribute is likely to have NaN values.',this)}}computeTangents(){let e=this.index,t=this.attributes;if(e===null||t.position===void 0||t.normal===void 0||t.uv===void 0){Xe("BufferGeometry: .computeTangents() failed. Missing required attributes (index, position, normal or uv)");return}let i=t.position,r=t.normal,s=t.uv,o=this.getAttribute("tangent");(o===void 0||o.count!==i.count)&&(o=new Pn(new Float32Array(4*i.count),4),this.setAttribute("tangent",o));let a=[],c=[];for(let p=0;p<i.count;p++)a[p]=new V,c[p]=new V;let l=new V,h=new V,m=new V,u=new He,g=new He,_=new He,x=new V,f=new V;function d(p,M,y){l.fromBufferAttribute(i,p),h.fromBufferAttribute(i,M),m.fromBufferAttribute(i,y),u.fromBufferAttribute(s,p),g.fromBufferAttribute(s,M),_.fromBufferAttribute(s,y),h.sub(l),m.sub(l),g.sub(u),_.sub(u);let C=1/(g.x*_.y-_.x*g.y);isFinite(C)&&(x.copy(h).multiplyScalar(_.y).addScaledVector(m,-g.y).multiplyScalar(C),f.copy(m).multiplyScalar(g.x).addScaledVector(h,-_.x).multiplyScalar(C),a[p].add(x),a[M].add(x),a[y].add(x),c[p].add(f),c[M].add(f),c[y].add(f))}let S=this.groups;S.length===0&&(S=[{start:0,count:e.count}]);for(let p=0,M=S.length;p<M;++p){let y=S[p],C=y.start,k=y.count;for(let L=C,U=C+k;L<U;L+=3)d(e.getX(L+0),e.getX(L+1),e.getX(L+2))}let P=new V,w=new V,T=new V,A=new V;function D(p){T.fromBufferAttribute(r,p),A.copy(T);let M=a[p];P.copy(M),P.sub(T.multiplyScalar(T.dot(M))).normalize(),w.crossVectors(A,M);let C=w.dot(c[p])<0?-1:1;o.setXYZW(p,P.x,P.y,P.z,C)}for(let p=0,M=S.length;p<M;++p){let y=S[p],C=y.start,k=y.count;for(let L=C,U=C+k;L<U;L+=3)D(e.getX(L+0)),D(e.getX(L+1)),D(e.getX(L+2))}this._transformed=!0}computeVertexNormals(){let e=this.index,t=this.getAttribute("position");if(t!==void 0){let i=this.getAttribute("normal");if(i===void 0||i.count!==t.count)i=new Pn(new Float32Array(t.count*3),3),this.setAttribute("normal",i);else for(let u=0,g=i.count;u<g;u++)i.setXYZ(u,0,0,0);let r=new V,s=new V,o=new V,a=new V,c=new V,l=new V,h=new V,m=new V;if(e)for(let u=0,g=e.count;u<g;u+=3){let _=e.getX(u+0),x=e.getX(u+1),f=e.getX(u+2);r.fromBufferAttribute(t,_),s.fromBufferAttribute(t,x),o.fromBufferAttribute(t,f),h.subVectors(o,s),m.subVectors(r,s),h.cross(m),a.fromBufferAttribute(i,_),c.fromBufferAttribute(i,x),l.fromBufferAttribute(i,f),a.add(h),c.add(h),l.add(h),i.setXYZ(_,a.x,a.y,a.z),i.setXYZ(x,c.x,c.y,c.z),i.setXYZ(f,l.x,l.y,l.z)}else for(let u=0,g=t.count;u<g;u+=3)r.fromBufferAttribute(t,u+0),s.fromBufferAttribute(t,u+1),o.fromBufferAttribute(t,u+2),h.subVectors(o,s),m.subVectors(r,s),h.cross(m),i.setXYZ(u+0,h.x,h.y,h.z),i.setXYZ(u+1,h.x,h.y,h.z),i.setXYZ(u+2,h.x,h.y,h.z);this.normalizeNormals(),i.needsUpdate=!0}}normalizeNormals(){let e=this.attributes.normal;for(let t=0,i=e.count;t<i;t++)un.fromBufferAttribute(e,t),un.normalize(),e.setXYZ(t,un.x,un.y,un.z)}toNonIndexed(){function e(a,c){let l=a.array,h=a.itemSize,m=a.normalized,u=new l.constructor(c.length*h),g=0,_=0;for(let x=0,f=c.length;x<f;x++){a.isInterleavedBufferAttribute?g=c[x]*a.data.stride+a.offset:g=c[x]*h;for(let d=0;d<h;d++)u[_++]=l[g++]}return new Pn(u,h,m)}if(this.index===null)return Ge("BufferGeometry.toNonIndexed(): BufferGeometry is already non-indexed."),this;let t=new n,i=this.index.array,r=this.attributes;for(let a in r){let c=r[a],l=e(c,i);t.setAttribute(a,l)}let s=this.morphAttributes;for(let a in s){let c=[],l=s[a];for(let h=0,m=l.length;h<m;h++){let u=l[h],g=e(u,i);c.push(g)}t.morphAttributes[a]=c}t.morphTargetsRelative=this.morphTargetsRelative;let o=this.groups;for(let a=0,c=o.length;a<c;a++){let l=o[a];t.addGroup(l.start,l.count,l.materialIndex)}return t}toJSON(){let e={metadata:{version:4.7,type:"BufferGeometry",generator:"BufferGeometry.toJSON"}};if(e.uuid=this.uuid,e.type=this.parameters!==void 0&&this._transformed===!0?"BufferGeometry":this.type,e.name=this.name,Object.keys(this.userData).length>0&&(e.userData=this.userData),this.parameters!==void 0&&this._transformed!==!0){let c=this.parameters;for(let l in c)c[l]!==void 0&&(e[l]=c[l]);return e}e.data={attributes:{}};let t=this.index;t!==null&&(e.data.index={type:t.array.constructor.name,array:Array.prototype.slice.call(t.array)});let i=this.attributes;for(let c in i){let l=i[c];e.data.attributes[c]=l.toJSON(e.data)}let r={},s=!1;for(let c in this.morphAttributes){let l=this.morphAttributes[c],h=[];for(let m=0,u=l.length;m<u;m++){let g=l[m];h.push(g.toJSON(e.data))}h.length>0&&(r[c]=h,s=!0)}s&&(e.data.morphAttributes=r,e.data.morphTargetsRelative=this.morphTargetsRelative);let o=this.groups;o.length>0&&(e.data.groups=JSON.parse(JSON.stringify(o)));let a=this.boundingSphere;return a!==null&&(e.data.boundingSphere=a.toJSON()),e}clone(){return new this.constructor().copy(this)}copy(e){this.index=null,this.attributes={},this.morphAttributes={},this.groups=[],this.boundingBox=null,this.boundingSphere=null;let t={};this.name=e.name;let i=e.index;i!==null&&this.setIndex(i.clone());let r=e.attributes;for(let l in r){let h=r[l];this.setAttribute(l,h.clone(t))}let s=e.morphAttributes;for(let l in s){let h=[],m=s[l];for(let u=0,g=m.length;u<g;u++)h.push(m[u].clone(t));this.morphAttributes[l]=h}this.morphTargetsRelative=e.morphTargetsRelative;let o=e.groups;for(let l=0,h=o.length;l<h;l++){let m=o[l];this.addGroup(m.start,m.count,m.materialIndex)}let a=e.boundingBox;a!==null&&(this.boundingBox=a.clone());let c=e.boundingSphere;return c!==null&&(this.boundingSphere=c.clone()),this.drawRange.start=e.drawRange.start,this.drawRange.count=e.drawRange.count,this.userData=e.userData,this._transformed=e._transformed,this}dispose(){this.dispatchEvent({type:"dispose"})}};var uc=new V,Wp=new V,Xp=new Ze,Un=class{constructor(e=new V(1,0,0),t=0){this.isPlane=!0,this.normal=e,this.constant=t}set(e,t){return this.normal.copy(e),this.constant=t,this}setComponents(e,t,i,r){return this.normal.set(e,t,i),this.constant=r,this}setFromNormalAndCoplanarPoint(e,t){return this.normal.copy(e),this.constant=-t.dot(this.normal),this}setFromCoplanarPoints(e,t,i){let r=uc.subVectors(i,t).cross(Wp.subVectors(e,t)).normalize();return this.setFromNormalAndCoplanarPoint(r,e),this}copy(e){return this.normal.copy(e.normal),this.constant=e.constant,this}normalize(){let e=1/this.normal.length();return this.normal.multiplyScalar(e),this.constant*=e,this}negate(){return this.constant*=-1,this.normal.negate(),this}distanceToPoint(e){return this.normal.dot(e)+this.constant}distanceToSphere(e){return this.distanceToPoint(e.center)-e.radius}projectPoint(e,t){return t.copy(e).addScaledVector(this.normal,-this.distanceToPoint(e))}intersectLine(e,t,i=!0){let r=e.delta(uc),s=this.normal.dot(r);if(s===0)return this.distanceToPoint(e.start)===0?t.copy(e.start):null;let o=-(e.start.dot(this.normal)+this.constant)/s;return i===!0&&(o<0||o>1)?null:t.copy(e.start).addScaledVector(r,o)}intersectsLine(e){let t=this.distanceToPoint(e.start),i=this.distanceToPoint(e.end);return t<0&&i>0||i<0&&t>0}intersectsBox(e){return e.intersectsPlane(this)}intersectsSphere(e){return e.intersectsPlane(this)}coplanarPoint(e){return e.copy(this.normal).multiplyScalar(-this.constant)}applyMatrix4(e,t){let i=t||Xp.getNormalMatrix(e),r=this.coplanarPoint(uc).applyMatrix4(e),s=this.normal.applyMatrix3(i).normalize();return this.constant=-r.dot(s),this}translate(e){return this.constant-=e.dot(this.normal),this}equals(e){return e.normal.equals(this.normal)&&e.constant===this.constant}clone(){return new this.constructor().copy(this)}toJSON(){return{normal:this.normal.toArray(),constant:this.constant}}fromJSON(e){return this.normal.fromArray(e.normal),this.constant=e.constant,this}},qp=0,fi=class extends ti{constructor(){super(),this.isMaterial=!0,Object.defineProperty(this,"id",{value:qp++}),this.uuid=hs(),this.name="",this.type="Material",this.blending=as,this.side=Zi,this.vertexColors=!1,this.opacity=1,this.transparent=!1,this.alphaHash=!1,this.blendSrc=Pc,this.blendDst=Ic,this.blendEquation=pr,this.blendSrcAlpha=null,this.blendDstAlpha=null,this.blendEquationAlpha=null,this.blendColor=new ze(0,0,0),this.blendAlpha=0,this.depthFunc=qr,this.depthTest=!0,this.depthWrite=!0,this.stencilWriteMask=255,this.stencilFunc=id,this.stencilRef=0,this.stencilFuncMask=255,this.stencilFail=Ko,this.stencilZFail=Ko,this.stencilZPass=Ko,this.stencilWrite=!1,this.clippingPlanes=null,this.clipIntersection=!1,this.clipShadows=!1,this.shadowSide=null,this.colorWrite=!0,this.precision=null,this.polygonOffset=!1,this.polygonOffsetFactor=0,this.polygonOffsetUnits=0,this.dithering=!1,this.alphaToCoverage=!1,this.premultipliedAlpha=!1,this.forceSinglePass=!1,this.allowOverride=!0,this.visible=!0,this.toneMapped=!0,this.userData={},this.version=0,this._alphaTest=0}get alphaTest(){return this._alphaTest}set alphaTest(e){this._alphaTest>0!=e>0&&this.version++,this._alphaTest=e}onBeforeRender(){}onBeforeCompile(){}customProgramCacheKey(){return this.onBeforeCompile.toString()}setValues(e){if(e!==void 0)for(let t in e){let i=e[t];if(i===void 0){Ge(`Material: parameter '${t}' has value of undefined.`);continue}let r=this[t];if(r===void 0){Ge(`Material: '${t}' is not a property of THREE.${this.type}.`);continue}r&&r.isColor?r.set(i):r&&r.isVector2&&i&&i.isVector2||r&&r.isEuler&&i&&i.isEuler||r&&r.isVector3&&i&&i.isVector3?r.copy(i):this[t]=i}}toJSON(e){let t=e===void 0||typeof e=="string";t&&(e={textures:{},images:{}});let i={metadata:{version:4.7,type:"Material",generator:"Material.toJSON"}};i.uuid=this.uuid,i.type=this.type,i.blending=this.blending,i.side=this.side,i.shadowSide=this.shadowSide,i.vertexColors=this.vertexColors,i.opacity=this.opacity,i.transparent=this.transparent,i.blendSrc=this.blendSrc,i.blendDst=this.blendDst,i.blendEquation=this.blendEquation,i.blendSrcAlpha=this.blendSrcAlpha,i.blendDstAlpha=this.blendDstAlpha,i.blendEquationAlpha=this.blendEquationAlpha,i.blendColor=this.blendColor.getHex(),i.blendAlpha=this.blendAlpha,i.depthFunc=this.depthFunc,i.depthTest=this.depthTest,i.depthWrite=this.depthWrite,i.colorWrite=this.colorWrite,i.clipIntersection=this.clipIntersection,i.clipShadows=this.clipShadows,i.stencilWriteMask=this.stencilWriteMask,i.stencilFunc=this.stencilFunc,i.stencilRef=this.stencilRef,i.stencilFuncMask=this.stencilFuncMask,i.stencilFail=this.stencilFail,i.stencilZFail=this.stencilZFail,i.stencilZPass=this.stencilZPass,i.stencilWrite=this.stencilWrite,i.polygonOffset=this.polygonOffset,i.polygonOffsetFactor=this.polygonOffsetFactor,i.polygonOffsetUnits=this.polygonOffsetUnits,i.dithering=this.dithering,i.alphaTest=this.alphaTest,i.alphaHash=this.alphaHash,i.alphaToCoverage=this.alphaToCoverage,i.premultipliedAlpha=this.premultipliedAlpha,i.forceSinglePass=this.forceSinglePass,i.allowOverride=this.allowOverride,i.visible=this.visible,i.toneMapped=this.toneMapped,i.name=this.name,this.color&&this.color.isColor&&(i.color=this.color.getHex()),this.roughness!==void 0&&(i.roughness=this.roughness),this.metalness!==void 0&&(i.metalness=this.metalness),this.sheen!==void 0&&(i.sheen=this.sheen),this.sheenColor&&this.sheenColor.isColor&&(i.sheenColor=this.sheenColor.getHex()),this.sheenRoughness!==void 0&&(i.sheenRoughness=this.sheenRoughness),this.emissive&&this.emissive.isColor&&(i.emissive=this.emissive.getHex()),this.emissiveIntensity!==void 0&&(i.emissiveIntensity=this.emissiveIntensity),this.specular&&this.specular.isColor&&(i.specular=this.specular.getHex()),this.specularIntensity!==void 0&&(i.specularIntensity=this.specularIntensity),this.specularColor&&this.specularColor.isColor&&(i.specularColor=this.specularColor.getHex()),this.shininess!==void 0&&(i.shininess=this.shininess),this.clearcoat!==void 0&&(i.clearcoat=this.clearcoat),this.clearcoatRoughness!==void 0&&(i.clearcoatRoughness=this.clearcoatRoughness),this.clearcoatMap&&this.clearcoatMap.isTexture&&(i.clearcoatMap=this.clearcoatMap.toJSON(e).uuid),this.clearcoatRoughnessMap&&this.clearcoatRoughnessMap.isTexture&&(i.clearcoatRoughnessMap=this.clearcoatRoughnessMap.toJSON(e).uuid),this.clearcoatNormalMap&&this.clearcoatNormalMap.isTexture&&(i.clearcoatNormalMap=this.clearcoatNormalMap.toJSON(e).uuid,i.clearcoatNormalScale=this.clearcoatNormalScale.toArray()),this.sheenColorMap&&this.sheenColorMap.isTexture&&(i.sheenColorMap=this.sheenColorMap.toJSON(e).uuid),this.sheenRoughnessMap&&this.sheenRoughnessMap.isTexture&&(i.sheenRoughnessMap=this.sheenRoughnessMap.toJSON(e).uuid),this.dispersion!==void 0&&(i.dispersion=this.dispersion),this.retroreflectivity!==void 0&&(i.retroreflectivity=this.retroreflectivity),this.iridescence!==void 0&&(i.iridescence=this.iridescence),this.iridescenceIOR!==void 0&&(i.iridescenceIOR=this.iridescenceIOR),this.iridescenceThicknessRange!==void 0&&(i.iridescenceThicknessRange=this.iridescenceThicknessRange),this.iridescenceMap&&this.iridescenceMap.isTexture&&(i.iridescenceMap=this.iridescenceMap.toJSON(e).uuid),this.iridescenceThicknessMap&&this.iridescenceThicknessMap.isTexture&&(i.iridescenceThicknessMap=this.iridescenceThicknessMap.toJSON(e).uuid),this.anisotropy!==void 0&&(i.anisotropy=this.anisotropy),this.anisotropyRotation!==void 0&&(i.anisotropyRotation=this.anisotropyRotation),this.anisotropyMap&&this.anisotropyMap.isTexture&&(i.anisotropyMap=this.anisotropyMap.toJSON(e).uuid),this.map&&this.map.isTexture&&(i.map=this.map.toJSON(e).uuid),this.matcap&&this.matcap.isTexture&&(i.matcap=this.matcap.toJSON(e).uuid),this.alphaMap&&this.alphaMap.isTexture&&(i.alphaMap=this.alphaMap.toJSON(e).uuid),this.lightMap&&this.lightMap.isTexture&&(i.lightMap=this.lightMap.toJSON(e).uuid,i.lightMapIntensity=this.lightMapIntensity),this.aoMap&&this.aoMap.isTexture&&(i.aoMap=this.aoMap.toJSON(e).uuid,i.aoMapIntensity=this.aoMapIntensity),this.bumpMap&&this.bumpMap.isTexture&&(i.bumpMap=this.bumpMap.toJSON(e).uuid,i.bumpScale=this.bumpScale),this.normalMap&&this.normalMap.isTexture&&(i.normalMap=this.normalMap.toJSON(e).uuid,i.normalMapType=this.normalMapType,i.normalScale=this.normalScale.toArray()),this.displacementMap&&this.displacementMap.isTexture&&(i.displacementMap=this.displacementMap.toJSON(e).uuid,i.displacementScale=this.displacementScale,i.displacementBias=this.displacementBias),this.roughnessMap&&this.roughnessMap.isTexture&&(i.roughnessMap=this.roughnessMap.toJSON(e).uuid),this.metalnessMap&&this.metalnessMap.isTexture&&(i.metalnessMap=this.metalnessMap.toJSON(e).uuid),this.emissiveMap&&this.emissiveMap.isTexture&&(i.emissiveMap=this.emissiveMap.toJSON(e).uuid),this.specularMap&&this.specularMap.isTexture&&(i.specularMap=this.specularMap.toJSON(e).uuid),this.specularIntensityMap&&this.specularIntensityMap.isTexture&&(i.specularIntensityMap=this.specularIntensityMap.toJSON(e).uuid),this.specularColorMap&&this.specularColorMap.isTexture&&(i.specularColorMap=this.specularColorMap.toJSON(e).uuid),this.envMap&&this.envMap.isTexture&&(i.envMap=this.envMap.toJSON(e).uuid,this.combine!==void 0&&(i.combine=this.combine)),this.envMapRotation!==void 0&&(i.envMapRotation=this.envMapRotation.toArray()),this.envMapIntensity!==void 0&&(i.envMapIntensity=this.envMapIntensity),this.reflectivity!==void 0&&(i.reflectivity=this.reflectivity),this.refractionRatio!==void 0&&(i.refractionRatio=this.refractionRatio),this.gradientMap&&this.gradientMap.isTexture&&(i.gradientMap=this.gradientMap.toJSON(e).uuid),this.transmission!==void 0&&(i.transmission=this.transmission),this.transmissionMap&&this.transmissionMap.isTexture&&(i.transmissionMap=this.transmissionMap.toJSON(e).uuid),this.thickness!==void 0&&(i.thickness=this.thickness),this.thicknessMap&&this.thicknessMap.isTexture&&(i.thicknessMap=this.thicknessMap.toJSON(e).uuid),this.attenuationDistance!==void 0&&(i.attenuationDistance=this.attenuationDistance),this.attenuationColor!==void 0&&(i.attenuationColor=this.attenuationColor.getHex()),this.size!==void 0&&(i.size=this.size),this.sizeAttenuation!==void 0&&(i.sizeAttenuation=this.sizeAttenuation),Array.isArray(this.clippingPlanes)&&this.clippingPlanes.length>0&&(i.clippingPlanes=this.clippingPlanes.map(s=>s.toJSON())),this.rotation!==void 0&&(i.rotation=this.rotation),this.depthPacking!==void 0&&(i.depthPacking=this.depthPacking),this.linewidth!==void 0&&(i.linewidth=this.linewidth),this.linecap!==void 0&&(i.linecap=this.linecap),this.linejoin!==void 0&&(i.linejoin=this.linejoin),this.dashSize!==void 0&&(i.dashSize=this.dashSize),this.gapSize!==void 0&&(i.gapSize=this.gapSize),this.scale!==void 0&&(i.scale=this.scale),this.wireframe!==void 0&&(i.wireframe=this.wireframe),this.wireframeLinewidth!==void 0&&(i.wireframeLinewidth=this.wireframeLinewidth),this.wireframeLinecap!==void 0&&(i.wireframeLinecap=this.wireframeLinecap),this.wireframeLinejoin!==void 0&&(i.wireframeLinejoin=this.wireframeLinejoin),this.flatShading!==void 0&&(i.flatShading=this.flatShading),this.fog!==void 0&&(i.fog=this.fog),Object.keys(this.userData).length>0&&(i.userData=this.userData);function r(s){let o=[];for(let a in s){let c=s[a];delete c.metadata,o.push(c)}return o}if(t){let s=r(e.textures),o=r(e.images);s.length>0&&(i.textures=s),o.length>0&&(i.images=o)}return i}fromJSON(e,t){if(e.uuid!==void 0&&(this.uuid=e.uuid),e.name!==void 0&&(this.name=e.name),e.color!==void 0&&this.color!==void 0&&this.color.setHex(e.color),e.roughness!==void 0&&(this.roughness=e.roughness),e.metalness!==void 0&&(this.metalness=e.metalness),e.sheen!==void 0&&(this.sheen=e.sheen),e.sheenColor!==void 0&&(this.sheenColor=new ze().setHex(e.sheenColor)),e.sheenRoughness!==void 0&&(this.sheenRoughness=e.sheenRoughness),e.emissive!==void 0&&this.emissive!==void 0&&this.emissive.setHex(e.emissive),e.specular!==void 0&&this.specular!==void 0&&this.specular.setHex(e.specular),e.specularIntensity!==void 0&&(this.specularIntensity=e.specularIntensity),e.specularColor!==void 0&&this.specularColor!==void 0&&this.specularColor.setHex(e.specularColor),e.shininess!==void 0&&(this.shininess=e.shininess),e.clearcoat!==void 0&&(this.clearcoat=e.clearcoat),e.clearcoatRoughness!==void 0&&(this.clearcoatRoughness=e.clearcoatRoughness),e.dispersion!==void 0&&(this.dispersion=e.dispersion),e.retroreflectivity!==void 0&&(this.retroreflectivity=e.retroreflectivity),e.iridescence!==void 0&&(this.iridescence=e.iridescence),e.iridescenceIOR!==void 0&&(this.iridescenceIOR=e.iridescenceIOR),e.iridescenceThicknessRange!==void 0&&(this.iridescenceThicknessRange=e.iridescenceThicknessRange),e.transmission!==void 0&&(this.transmission=e.transmission),e.thickness!==void 0&&(this.thickness=e.thickness),e.attenuationDistance!==void 0&&(this.attenuationDistance=e.attenuationDistance),e.attenuationColor!==void 0&&this.attenuationColor!==void 0&&this.attenuationColor.setHex(e.attenuationColor),e.anisotropy!==void 0&&(this.anisotropy=e.anisotropy),e.anisotropyRotation!==void 0&&(this.anisotropyRotation=e.anisotropyRotation),e.fog!==void 0&&(this.fog=e.fog),e.flatShading!==void 0&&(this.flatShading=e.flatShading),e.blending!==void 0&&(this.blending=e.blending),e.combine!==void 0&&(this.combine=e.combine),e.side!==void 0&&(this.side=e.side),e.shadowSide!==void 0&&(this.shadowSide=e.shadowSide),e.opacity!==void 0&&(this.opacity=e.opacity),e.transparent!==void 0&&(this.transparent=e.transparent),e.alphaTest!==void 0&&(this.alphaTest=e.alphaTest),e.alphaHash!==void 0&&(this.alphaHash=e.alphaHash),e.depthFunc!==void 0&&(this.depthFunc=e.depthFunc),e.depthTest!==void 0&&(this.depthTest=e.depthTest),e.depthWrite!==void 0&&(this.depthWrite=e.depthWrite),e.colorWrite!==void 0&&(this.colorWrite=e.colorWrite),e.clippingPlanes!==void 0&&(this.clippingPlanes=e.clippingPlanes.map(i=>new Un().fromJSON(i))),e.clipIntersection!==void 0&&(this.clipIntersection=e.clipIntersection),e.clipShadows!==void 0&&(this.clipShadows=e.clipShadows),e.depthPacking!==void 0&&(this.depthPacking=e.depthPacking),e.blendSrc!==void 0&&(this.blendSrc=e.blendSrc),e.blendDst!==void 0&&(this.blendDst=e.blendDst),e.blendEquation!==void 0&&(this.blendEquation=e.blendEquation),e.blendSrcAlpha!==void 0&&(this.blendSrcAlpha=e.blendSrcAlpha),e.blendDstAlpha!==void 0&&(this.blendDstAlpha=e.blendDstAlpha),e.blendEquationAlpha!==void 0&&(this.blendEquationAlpha=e.blendEquationAlpha),e.blendColor!==void 0&&this.blendColor!==void 0&&this.blendColor.setHex(e.blendColor),e.blendAlpha!==void 0&&(this.blendAlpha=e.blendAlpha),e.stencilWriteMask!==void 0&&(this.stencilWriteMask=e.stencilWriteMask),e.stencilFunc!==void 0&&(this.stencilFunc=e.stencilFunc),e.stencilRef!==void 0&&(this.stencilRef=e.stencilRef),e.stencilFuncMask!==void 0&&(this.stencilFuncMask=e.stencilFuncMask),e.stencilFail!==void 0&&(this.stencilFail=e.stencilFail),e.stencilZFail!==void 0&&(this.stencilZFail=e.stencilZFail),e.stencilZPass!==void 0&&(this.stencilZPass=e.stencilZPass),e.stencilWrite!==void 0&&(this.stencilWrite=e.stencilWrite),e.wireframe!==void 0&&(this.wireframe=e.wireframe),e.wireframeLinewidth!==void 0&&(this.wireframeLinewidth=e.wireframeLinewidth),e.wireframeLinecap!==void 0&&(this.wireframeLinecap=e.wireframeLinecap),e.wireframeLinejoin!==void 0&&(this.wireframeLinejoin=e.wireframeLinejoin),e.rotation!==void 0&&(this.rotation=e.rotation),e.linewidth!==void 0&&(this.linewidth=e.linewidth),e.linecap!==void 0&&(this.linecap=e.linecap),e.linejoin!==void 0&&(this.linejoin=e.linejoin),e.dashSize!==void 0&&(this.dashSize=e.dashSize),e.gapSize!==void 0&&(this.gapSize=e.gapSize),e.scale!==void 0&&(this.scale=e.scale),e.polygonOffset!==void 0&&(this.polygonOffset=e.polygonOffset),e.polygonOffsetFactor!==void 0&&(this.polygonOffsetFactor=e.polygonOffsetFactor),e.polygonOffsetUnits!==void 0&&(this.polygonOffsetUnits=e.polygonOffsetUnits),e.dithering!==void 0&&(this.dithering=e.dithering),e.alphaToCoverage!==void 0&&(this.alphaToCoverage=e.alphaToCoverage),e.premultipliedAlpha!==void 0&&(this.premultipliedAlpha=e.premultipliedAlpha),e.forceSinglePass!==void 0&&(this.forceSinglePass=e.forceSinglePass),e.allowOverride!==void 0&&(this.allowOverride=e.allowOverride),e.visible!==void 0&&(this.visible=e.visible),e.toneMapped!==void 0&&(this.toneMapped=e.toneMapped),e.userData!==void 0&&(this.userData=e.userData),e.vertexColors!==void 0&&(typeof e.vertexColors=="number"?this.vertexColors=e.vertexColors>0:this.vertexColors=e.vertexColors),e.size!==void 0&&(this.size=e.size),e.sizeAttenuation!==void 0&&(this.sizeAttenuation=e.sizeAttenuation),e.map!==void 0&&(this.map=t[e.map]||null),e.matcap!==void 0&&(this.matcap=t[e.matcap]||null),e.alphaMap!==void 0&&(this.alphaMap=t[e.alphaMap]||null),e.bumpMap!==void 0&&(this.bumpMap=t[e.bumpMap]||null),e.bumpScale!==void 0&&(this.bumpScale=e.bumpScale),e.normalMap!==void 0&&(this.normalMap=t[e.normalMap]||null),e.normalMapType!==void 0&&(this.normalMapType=e.normalMapType),e.normalScale!==void 0){let i=e.normalScale;Array.isArray(i)===!1&&(i=[i,i]),this.normalScale=new He().fromArray(i)}return e.displacementMap!==void 0&&(this.displacementMap=t[e.displacementMap]||null),e.displacementScale!==void 0&&(this.displacementScale=e.displacementScale),e.displacementBias!==void 0&&(this.displacementBias=e.displacementBias),e.roughnessMap!==void 0&&(this.roughnessMap=t[e.roughnessMap]||null),e.metalnessMap!==void 0&&(this.metalnessMap=t[e.metalnessMap]||null),e.emissiveMap!==void 0&&(this.emissiveMap=t[e.emissiveMap]||null),e.emissiveIntensity!==void 0&&(this.emissiveIntensity=e.emissiveIntensity),e.specularMap!==void 0&&(this.specularMap=t[e.specularMap]||null),e.specularIntensityMap!==void 0&&(this.specularIntensityMap=t[e.specularIntensityMap]||null),e.specularColorMap!==void 0&&(this.specularColorMap=t[e.specularColorMap]||null),e.envMap!==void 0&&(this.envMap=t[e.envMap]||null),e.envMapRotation!==void 0&&this.envMapRotation.fromArray(e.envMapRotation),e.envMapIntensity!==void 0&&(this.envMapIntensity=e.envMapIntensity),e.reflectivity!==void 0&&(this.reflectivity=e.reflectivity),e.refractionRatio!==void 0&&(this.refractionRatio=e.refractionRatio),e.lightMap!==void 0&&(this.lightMap=t[e.lightMap]||null),e.lightMapIntensity!==void 0&&(this.lightMapIntensity=e.lightMapIntensity),e.aoMap!==void 0&&(this.aoMap=t[e.aoMap]||null),e.aoMapIntensity!==void 0&&(this.aoMapIntensity=e.aoMapIntensity),e.gradientMap!==void 0&&(this.gradientMap=t[e.gradientMap]||null),e.clearcoatMap!==void 0&&(this.clearcoatMap=t[e.clearcoatMap]||null),e.clearcoatRoughnessMap!==void 0&&(this.clearcoatRoughnessMap=t[e.clearcoatRoughnessMap]||null),e.clearcoatNormalMap!==void 0&&(this.clearcoatNormalMap=t[e.clearcoatNormalMap]||null),e.clearcoatNormalScale!==void 0&&(this.clearcoatNormalScale=new He().fromArray(e.clearcoatNormalScale)),e.iridescenceMap!==void 0&&(this.iridescenceMap=t[e.iridescenceMap]||null),e.iridescenceThicknessMap!==void 0&&(this.iridescenceThicknessMap=t[e.iridescenceThicknessMap]||null),e.transmissionMap!==void 0&&(this.transmissionMap=t[e.transmissionMap]||null),e.thicknessMap!==void 0&&(this.thicknessMap=t[e.thicknessMap]||null),e.anisotropyMap!==void 0&&(this.anisotropyMap=t[e.anisotropyMap]||null),e.sheenColorMap!==void 0&&(this.sheenColorMap=t[e.sheenColorMap]||null),e.sheenRoughnessMap!==void 0&&(this.sheenRoughnessMap=t[e.sheenRoughnessMap]||null),this}clone(){return new this.constructor().copy(this)}copy(e){this.name=e.name,this.blending=e.blending,this.side=e.side,this.vertexColors=e.vertexColors,this.opacity=e.opacity,this.transparent=e.transparent,this.blendSrc=e.blendSrc,this.blendDst=e.blendDst,this.blendEquation=e.blendEquation,this.blendSrcAlpha=e.blendSrcAlpha,this.blendDstAlpha=e.blendDstAlpha,this.blendEquationAlpha=e.blendEquationAlpha,this.blendColor.copy(e.blendColor),this.blendAlpha=e.blendAlpha,this.depthFunc=e.depthFunc,this.depthTest=e.depthTest,this.depthWrite=e.depthWrite,this.stencilWriteMask=e.stencilWriteMask,this.stencilFunc=e.stencilFunc,this.stencilRef=e.stencilRef,this.stencilFuncMask=e.stencilFuncMask,this.stencilFail=e.stencilFail,this.stencilZFail=e.stencilZFail,this.stencilZPass=e.stencilZPass,this.stencilWrite=e.stencilWrite;let t=e.clippingPlanes,i=null;if(t!==null){let r=t.length;i=new Array(r);for(let s=0;s!==r;++s)i[s]=t[s].clone()}return this.clippingPlanes=i,this.clipIntersection=e.clipIntersection,this.clipShadows=e.clipShadows,this.shadowSide=e.shadowSide,this.colorWrite=e.colorWrite,this.precision=e.precision,this.polygonOffset=e.polygonOffset,this.polygonOffsetFactor=e.polygonOffsetFactor,this.polygonOffsetUnits=e.polygonOffsetUnits,this.dithering=e.dithering,this.alphaTest=e.alphaTest,this.alphaHash=e.alphaHash,this.alphaToCoverage=e.alphaToCoverage,this.premultipliedAlpha=e.premultipliedAlpha,this.forceSinglePass=e.forceSinglePass,this.allowOverride=e.allowOverride,this.visible=e.visible,this.toneMapped=e.toneMapped,this.userData=JSON.parse(JSON.stringify(e.userData)),this}dispose(){this.dispatchEvent({type:"dispose"})}set needsUpdate(e){e===!0&&this.version++}};var wi=new V,hc=new V,Po=new V,Io=new V,Ri=class{constructor(e=new V,t=new V(0,0,-1)){this.origin=e,this.direction=t}set(e,t){return this.origin.copy(e),this.direction.copy(t),this}copy(e){return this.origin.copy(e.origin),this.direction.copy(e.direction),this}at(e,t){return t.copy(this.origin).addScaledVector(this.direction,e)}lookAt(e){return this.direction.copy(e).sub(this.origin).normalize(),this}recast(e){return this.origin.copy(this.at(e,wi)),this}closestPointToPoint(e,t){t.subVectors(e,this.origin);let i=t.dot(this.direction);return i<0?t.copy(this.origin):t.copy(this.origin).addScaledVector(this.direction,i)}distanceToPoint(e){return Math.sqrt(this.distanceSqToPoint(e))}distanceSqToPoint(e){let t=wi.subVectors(e,this.origin).dot(this.direction);return t<0?this.origin.distanceToSquared(e):(wi.copy(this.origin).addScaledVector(this.direction,t),wi.distanceToSquared(e))}distanceSqToSegment(e,t,i,r){hc.copy(e).add(t).multiplyScalar(.5),Po.copy(t).sub(e).normalize(),Io.copy(this.origin).sub(hc);let s=e.distanceTo(t)*.5,o=-this.direction.dot(Po),a=Io.dot(this.direction),c=-Io.dot(Po),l=Io.lengthSq(),h=Math.abs(1-o*o),m,u,g,_;if(h>0)if(m=o*c-a,u=o*a-c,_=s*h,m>=0)if(u>=-_)if(u<=_){let x=1/h;m*=x,u*=x,g=m*(m+o*u+2*a)+u*(o*m+u+2*c)+l}else u=s,m=Math.max(0,-(o*u+a)),g=-m*m+u*(u+2*c)+l;else u=-s,m=Math.max(0,-(o*u+a)),g=-m*m+u*(u+2*c)+l;else u<=-_?(m=Math.max(0,-(-o*s+a)),u=m>0?-s:Math.min(Math.max(-s,-c),s),g=-m*m+u*(u+2*c)+l):u<=_?(m=0,u=Math.min(Math.max(-s,-c),s),g=u*(u+2*c)+l):(m=Math.max(0,-(o*s+a)),u=m>0?s:Math.min(Math.max(-s,-c),s),g=-m*m+u*(u+2*c)+l);else u=o>0?-s:s,m=Math.max(0,-(o*u+a)),g=-m*m+u*(u+2*c)+l;return i&&i.copy(this.origin).addScaledVector(this.direction,m),r&&r.copy(hc).addScaledVector(Po,u),g}intersectSphere(e,t){if(e.radius<0)return null;wi.subVectors(e.center,this.origin);let i=wi.dot(this.direction),r=wi.dot(wi)-i*i,s=e.radius*e.radius;if(r>s)return null;let o=Math.sqrt(s-r),a=i-o,c=i+o;return c<0?null:a<0?this.at(c,t):this.at(a,t)}intersectsSphere(e){return e.radius<0?!1:this.distanceSqToPoint(e.center)<=e.radius*e.radius}distanceToPlane(e){let t=e.normal.dot(this.direction);if(t===0)return e.distanceToPoint(this.origin)===0?0:null;let i=-(this.origin.dot(e.normal)+e.constant)/t;return i>=0?i:null}intersectPlane(e,t){let i=this.distanceToPlane(e);return i===null?null:this.at(i,t)}intersectsPlane(e){let t=e.distanceToPoint(this.origin);return t===0||e.normal.dot(this.direction)*t<0}intersectBox(e,t){let i,r,s,o,a,c,l=1/this.direction.x,h=1/this.direction.y,m=1/this.direction.z,u=this.origin;return l>=0?(i=(e.min.x-u.x)*l,r=(e.max.x-u.x)*l):(i=(e.max.x-u.x)*l,r=(e.min.x-u.x)*l),h>=0?(s=(e.min.y-u.y)*h,o=(e.max.y-u.y)*h):(s=(e.max.y-u.y)*h,o=(e.min.y-u.y)*h),i>o||s>r||((s>i||isNaN(i))&&(i=s),(o<r||isNaN(r))&&(r=o),m>=0?(a=(e.min.z-u.z)*m,c=(e.max.z-u.z)*m):(a=(e.max.z-u.z)*m,c=(e.min.z-u.z)*m),i>c||a>r)||((a>i||i!==i)&&(i=a),(c<r||r!==r)&&(r=c),r<0)?null:this.at(i>=0?i:r,t)}intersectsBox(e){return this.intersectBox(e,wi)!==null}intersectTriangle(e,t,i,r,s){let o=this.origin,a=this.direction,c=a.x,l=a.y,h=a.z,m=e.x-o.x,u=e.y-o.y,g=e.z-o.z,_=t.x-o.x,x=t.y-o.y,f=t.z-o.z,d=i.x-o.x,S=i.y-o.y,P=i.z-o.z,w=Math.abs(c),T=Math.abs(l),A=Math.abs(h),D,p,M,y,C,k,L,U,F,H,q,ee;if(w>=T&&w>=A?(M=c,k=m,F=_,ee=d,c>=0?(D=l,p=h,y=u,C=g,L=x,U=f,H=S,q=P):(D=h,p=l,y=g,C=u,L=f,U=x,H=P,q=S)):T>=A?(M=l,k=u,F=x,ee=S,l>=0?(D=h,p=c,y=g,C=m,L=f,U=_,H=P,q=d):(D=c,p=h,y=m,C=g,L=_,U=f,H=d,q=P)):(M=h,k=g,F=f,ee=P,h>=0?(D=c,p=l,y=m,C=u,L=_,U=x,H=d,q=S):(D=l,p=c,y=u,C=m,L=x,U=_,H=S,q=d)),M===0)return null;let J=D/M,ne=p/M,ce=1/M,pe=y-J*k,me=C-ne*k,qe=L-J*F,Le=U-ne*F,Je=H-J*ee,se=q-ne*ee,le=Je*Le-se*qe,Se=pe*se-me*Je,ke=qe*me-Le*pe;if(r){if(le<0||Se<0||ke<0)return null}else if((le<0||Se<0||ke<0)&&(le>0||Se>0||ke>0))return null;let ye=le+Se+ke;if(ye===0)return null;let je=ce*(le*k+Se*F+ke*ee);return(ye>0?je<0:je>0)?null:this.at(je/ye,s)}applyMatrix4(e){return this.origin.applyMatrix4(e),this.direction.transformDirection(e),this}equals(e){return e.origin.equals(this.origin)&&e.direction.equals(this.direction)}clone(){return new this.constructor().copy(this)}},on=class extends fi{constructor(e){super(),this.isMeshBasicMaterial=!0,this.type="MeshBasicMaterial",this.color=new ze(16777215),this.map=null,this.lightMap=null,this.lightMapIntensity=1,this.aoMap=null,this.aoMapIntensity=1,this.specularMap=null,this.alphaMap=null,this.envMap=null,this.envMapRotation=new Ai,this.combine=Lc,this.reflectivity=1,this.refractionRatio=.98,this.wireframe=!1,this.wireframeLinewidth=1,this.wireframeLinecap="round",this.wireframeLinejoin="round",this.fog=!0,this.setValues(e)}copy(e){return super.copy(e),this.color.copy(e.color),this.map=e.map,this.lightMap=e.lightMap,this.lightMapIntensity=e.lightMapIntensity,this.aoMap=e.aoMap,this.aoMapIntensity=e.aoMapIntensity,this.specularMap=e.specularMap,this.alphaMap=e.alphaMap,this.envMap=e.envMap,this.envMapRotation.copy(e.envMapRotation),this.combine=e.combine,this.reflectivity=e.reflectivity,this.refractionRatio=e.refractionRatio,this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this.wireframeLinecap=e.wireframeLinecap,this.wireframeLinejoin=e.wireframeLinejoin,this.fog=e.fog,this}},mh=new It,ar=new Ri,Lo=new Hi,gh=new V,ko=new V,Do=new V,No=new V,dc=new V,Uo=new V,xh=new V,Fo=new V,xt=class extends dn{constructor(e=new Xt,t=new on){super(),this.isMesh=!0,this.type="Mesh",this.geometry=e,this.material=t,this.morphTargetDictionary=void 0,this.morphTargetInfluences=void 0,this.count=1,this.updateMorphTargets()}copy(e,t){return super.copy(e,t),e.morphTargetInfluences!==void 0&&(this.morphTargetInfluences=e.morphTargetInfluences.slice()),e.morphTargetDictionary!==void 0&&(this.morphTargetDictionary=Object.assign({},e.morphTargetDictionary)),this.material=Array.isArray(e.material)?e.material.slice():e.material,this.geometry=e.geometry,this}updateMorphTargets(){let t=this.geometry.morphAttributes,i=Object.keys(t);if(i.length>0){let r=t[i[0]];if(r!==void 0){this.morphTargetInfluences=[],this.morphTargetDictionary={};for(let s=0,o=r.length;s<o;s++){let a=r[s].name||String(s);this.morphTargetInfluences.push(0),this.morphTargetDictionary[a]=s}}}}getVertexPosition(e,t){let i=this.geometry,r=i.attributes.position,s=i.morphAttributes.position,o=i.morphTargetsRelative;t.fromBufferAttribute(r,e);let a=this.morphTargetInfluences;if(s&&a){Uo.set(0,0,0);for(let c=0,l=s.length;c<l;c++){let h=a[c],m=s[c];h!==0&&(dc.fromBufferAttribute(m,e),o?Uo.addScaledVector(dc,h):Uo.addScaledVector(dc.sub(t),h))}t.add(Uo)}return t}intersectsFrustum(e){return e.intersectsObject(this)}raycast(e,t){let i=this.geometry,r=this.material,s=this.matrixWorld;r!==void 0&&(i.boundingSphere===null&&i.computeBoundingSphere(),Lo.copy(i.boundingSphere),Lo.applyMatrix4(s),ar.copy(e.ray).recast(e.near),!(Lo.containsPoint(ar.origin)===!1&&(ar.intersectSphere(Lo,gh)===null||ar.origin.distanceToSquared(gh)>(e.far-e.near)**2))&&(mh.copy(s).invert(),ar.copy(e.ray).applyMatrix4(mh),!(i.boundingBox!==null&&ar.intersectsBox(i.boundingBox)===!1)&&this._computeIntersections(e,t,ar)))}_computeIntersections(e,t,i){let r,s=this.geometry,o=this.material,a=s.index,c=s.attributes.position,l=s.attributes.uv,h=s.attributes.uv1,m=s.attributes.normal,u=s.groups,g=s.drawRange;if(a!==null)if(Array.isArray(o))for(let _=0,x=u.length;_<x;_++){let f=u[_],d=o[f.materialIndex],S=Math.max(f.start,g.start),P=Math.min(a.count,Math.min(f.start+f.count,g.start+g.count));for(let w=S,T=P;w<T;w+=3){let A=a.getX(w),D=a.getX(w+1),p=a.getX(w+2);r=Oo(this,d,e,i,l,h,m,A,D,p),r&&(r.faceIndex=Math.floor(w/3),r.face.materialIndex=f.materialIndex,t.push(r))}}else{let _=Math.max(0,g.start),x=Math.min(a.count,g.start+g.count);for(let f=_,d=x;f<d;f+=3){let S=a.getX(f),P=a.getX(f+1),w=a.getX(f+2);r=Oo(this,o,e,i,l,h,m,S,P,w),r&&(r.faceIndex=Math.floor(f/3),t.push(r))}}else if(c!==void 0)if(Array.isArray(o))for(let _=0,x=u.length;_<x;_++){let f=u[_],d=o[f.materialIndex],S=Math.max(f.start,g.start),P=Math.min(c.count,Math.min(f.start+f.count,g.start+g.count));for(let w=S,T=P;w<T;w+=3){let A=w,D=w+1,p=w+2;r=Oo(this,d,e,i,l,h,m,A,D,p),r&&(r.faceIndex=Math.floor(w/3),r.face.materialIndex=f.materialIndex,t.push(r))}}else{let _=Math.max(0,g.start),x=Math.min(c.count,g.start+g.count);for(let f=_,d=x;f<d;f+=3){let S=f,P=f+1,w=f+2;r=Oo(this,o,e,i,l,h,m,S,P,w),r&&(r.faceIndex=Math.floor(f/3),t.push(r))}}}};function $p(n,e,t,i,r,s,o,a){let c;if(e.side===Rn?c=i.intersectTriangle(o,s,r,!0,a):c=i.intersectTriangle(r,s,o,e.side===Zi,a),c===null)return null;Fo.copy(a),Fo.applyMatrix4(n.matrixWorld);let l=t.ray.origin.distanceTo(Fo);return l<t.near||l>t.far?null:{distance:l,point:Fo.clone(),object:n}}function Oo(n,e,t,i,r,s,o,a,c,l){n.getVertexPosition(a,ko),n.getVertexPosition(c,Do),n.getVertexPosition(l,No);let h=$p(n,e,t,i,ko,Do,No,xh);if(h){let m=new V;Ei.getBarycoord(xh,ko,Do,No,m),r&&(h.uv=Ei.getInterpolatedAttribute(r,a,c,l,m,new He)),s&&(h.uv1=Ei.getInterpolatedAttribute(s,a,c,l,m,new He)),o&&(h.normal=Ei.getInterpolatedAttribute(o,a,c,l,m,new V),h.normal.dot(i.direction)>0&&h.normal.multiplyScalar(-1));let u={a,b:c,c:l,normal:new V,materialIndex:0};Ei.getNormal(ko,Do,No,u.normal),h.face=u,h.barycoord=m}return h}var ca=class extends An{constructor(e=null,t=1,i=1,r,s,o,a,c,l=hn,h=hn,m,u){super(null,o,a,c,l,h,r,s,m,u),this.isDataTexture=!0,this.image={data:e,width:t,height:i},this.generateMipmaps=!1,this.flipY=!1,this.unpackAlignment=1}};var lr=new Hi,Yp=new He(.5,.5),Bo=new V,Qr=class{constructor(e=new Un,t=new Un,i=new Un,r=new Un,s=new Un,o=new Un){this.planes=[e,t,i,r,s,o]}set(e,t,i,r,s,o){let a=this.planes;return a[0].copy(e),a[1].copy(t),a[2].copy(i),a[3].copy(r),a[4].copy(s),a[5].copy(o),this}copy(e){let t=this.planes;for(let i=0;i<6;i++)t[i].copy(e.planes[i]);return this}setFromProjectionMatrix(e,t=ei,i=!1){let r=this.planes,s=e.elements,o=s[0],a=s[1],c=s[2],l=s[3],h=s[4],m=s[5],u=s[6],g=s[7],_=s[8],x=s[9],f=s[10],d=s[11],S=s[12],P=s[13],w=s[14],T=s[15];if(r[0].setComponents(l-o,g-h,d-_,T-S).normalize(),r[1].setComponents(l+o,g+h,d+_,T+S).normalize(),r[2].setComponents(l+a,g+m,d+x,T+P).normalize(),r[3].setComponents(l-a,g-m,d-x,T-P).normalize(),i)r[4].setComponents(c,u,f,w).normalize(),r[5].setComponents(l-c,g-u,d-f,T-w).normalize();else if(r[4].setComponents(l-c,g-u,d-f,T-w).normalize(),t===ei)r[5].setComponents(l+c,g+u,d+f,T+w).normalize();else if(t===$r)r[5].setComponents(c,u,f,w).normalize();else throw new Error("THREE.Frustum.setFromProjectionMatrix(): Invalid coordinate system: "+t);return this}intersectsObject(e){if(e.boundingSphere!==void 0)e.boundingSphere===null&&e.computeBoundingSphere(),lr.copy(e.boundingSphere).applyMatrix4(e.matrixWorld);else{let t=e.geometry;t.boundingSphere===null&&t.computeBoundingSphere(),lr.copy(t.boundingSphere).applyMatrix4(e.matrixWorld)}return this.intersectsSphere(lr)}intersectsSprite(e){lr.center.set(0,0,0);let t=Yp.distanceTo(e.center);return lr.radius=.7071067811865476+t,lr.applyMatrix4(e.matrixWorld),this.intersectsSphere(lr)}intersectsSphere(e){let t=this.planes,i=e.center,r=-e.radius;for(let s=0;s<6;s++)if(t[s].distanceToPoint(i)<r)return!1;return!0}intersectsBox(e){let t=this.planes;for(let i=0;i<6;i++){let r=t[i];if(Bo.x=r.normal.x>0?e.max.x:e.min.x,Bo.y=r.normal.y>0?e.max.y:e.min.y,Bo.z=r.normal.z>0?e.max.z:e.min.z,r.distanceToPoint(Bo)<0)return!1}return!0}containsPoint(e){let t=this.planes;for(let i=0;i<6;i++)if(t[i].distanceToPoint(e)<0)return!1;return!0}clone(){return new this.constructor().copy(this)}};var hr=class extends fi{constructor(e){super(),this.isLineBasicMaterial=!0,this.type="LineBasicMaterial",this.color=new ze(16777215),this.map=null,this.linewidth=1,this.linecap="round",this.linejoin="round",this.fog=!0,this.setValues(e)}copy(e){return super.copy(e),this.color.copy(e.color),this.map=e.map,this.linewidth=e.linewidth,this.linecap=e.linecap,this.linejoin=e.linejoin,this.fog=e.fog,this}},ua=new V,ha=new V,vh=new It,Cs=new Ri,zo=new Hi,fc=new V,_h=new V,da=class extends dn{constructor(e=new Xt,t=new hr){super(),this.isLine=!0,this.type="Line",this.geometry=e,this.material=t,this.morphTargetDictionary=void 0,this.morphTargetInfluences=void 0,this.updateMorphTargets()}copy(e,t){return super.copy(e,t),this.material=Array.isArray(e.material)?e.material.slice():e.material,this.geometry=e.geometry,this}computeLineDistances(){let e=this.geometry;if(e.index===null){let t=e.attributes.position,i=[0];for(let r=1,s=t.count;r<s;r++)ua.fromBufferAttribute(t,r-1),ha.fromBufferAttribute(t,r),i[r]=i[r-1],i[r]+=ua.distanceTo(ha);e.setAttribute("lineDistance",new ct(i,1))}else Ge("Line.computeLineDistances(): Computation only possible with non-indexed BufferGeometry.");return this}intersectsFrustum(e){return e.intersectsObject(this)}raycast(e,t){let i=this.geometry,r=this.matrixWorld,s=e.params.Line.threshold,o=i.drawRange;if(i.boundingSphere===null&&i.computeBoundingSphere(),zo.copy(i.boundingSphere),zo.applyMatrix4(r),zo.radius+=s,e.ray.intersectsSphere(zo)===!1)return;vh.copy(r).invert(),Cs.copy(e.ray).applyMatrix4(vh);let a=s/((this.scale.x+this.scale.y+this.scale.z)/3),c=a*a,l=this.isLineSegments?2:1,h=i.index,u=i.attributes.position;if(h!==null){let g=Math.max(0,o.start),_=Math.min(h.count,o.start+o.count);for(let x=g,f=_-1;x<f;x+=l){let d=h.getX(x),S=h.getX(x+1),P=Ho(this,e,Cs,c,d,S,x);P&&t.push(P)}if(this.isLineLoop){let x=h.getX(_-1),f=h.getX(g),d=Ho(this,e,Cs,c,x,f,_-1);d&&t.push(d)}}else{let g=Math.max(0,o.start),_=Math.min(u.count,o.start+o.count);for(let x=g,f=_-1;x<f;x+=l){let d=Ho(this,e,Cs,c,x,x+1,x);d&&t.push(d)}if(this.isLineLoop){let x=Ho(this,e,Cs,c,_-1,g,_-1);x&&t.push(x)}}}updateMorphTargets(){let t=this.geometry.morphAttributes,i=Object.keys(t);if(i.length>0){let r=t[i[0]];if(r!==void 0){this.morphTargetInfluences=[],this.morphTargetDictionary={};for(let s=0,o=r.length;s<o;s++){let a=r[s].name||String(s);this.morphTargetInfluences.push(0),this.morphTargetDictionary[a]=s}}}}};function Ho(n,e,t,i,r,s,o){let a=n.geometry.attributes.position;if(ua.fromBufferAttribute(a,r),ha.fromBufferAttribute(a,s),t.distanceSqToSegment(ua,ha,fc,_h)>i)return;fc.applyMatrix4(n.matrixWorld);let l=e.ray.origin.distanceTo(fc);if(!(l<e.near||l>e.far))return{distance:l,point:_h.clone().applyMatrix4(n.matrixWorld),index:o,face:null,faceIndex:null,barycoord:null,object:n}}var yh=new V,bh=new V,es=class extends da{constructor(e,t){super(e,t),this.isLineSegments=!0,this.type="LineSegments"}computeLineDistances(){let e=this.geometry;if(e.index===null){let t=e.attributes.position,i=[];for(let r=0,s=t.count;r<s;r+=2)yh.fromBufferAttribute(t,r),bh.fromBufferAttribute(t,r+1),i[r]=r===0?0:i[r-1],i[r+1]=i[r]+yh.distanceTo(bh);e.setAttribute("lineDistance",new ct(i,1))}else Ge("LineSegments.computeLineDistances(): Computation only possible with non-indexed BufferGeometry.");return this}};var ts=class extends fi{constructor(e){super(),this.isPointsMaterial=!0,this.type="PointsMaterial",this.color=new ze(16777215),this.map=null,this.alphaMap=null,this.size=1,this.sizeAttenuation=!0,this.fog=!0,this.setValues(e)}copy(e){return super.copy(e),this.color.copy(e.color),this.map=e.map,this.alphaMap=e.alphaMap,this.size=e.size,this.sizeAttenuation=e.sizeAttenuation,this.fog=e.fog,this}},Mh=new It,bc=new Ri,Vo=new Hi,Go=new V,Bs=class extends dn{constructor(e=new Xt,t=new ts){super(),this.isPoints=!0,this.type="Points",this.geometry=e,this.material=t,this.morphTargetDictionary=void 0,this.morphTargetInfluences=void 0,this.updateMorphTargets()}copy(e,t){return super.copy(e,t),this.material=Array.isArray(e.material)?e.material.slice():e.material,this.geometry=e.geometry,this}intersectsFrustum(e){return e.intersectsObject(this)}raycast(e,t){let i=this.geometry,r=this.matrixWorld,s=e.params.Points.threshold,o=i.drawRange;if(i.boundingSphere===null&&i.computeBoundingSphere(),Vo.copy(i.boundingSphere),Vo.applyMatrix4(r),Vo.radius+=s,e.ray.intersectsSphere(Vo)===!1)return;Mh.copy(r).invert(),bc.copy(e.ray).applyMatrix4(Mh);let a=s/((this.scale.x+this.scale.y+this.scale.z)/3),c=a*a,l=i.index,m=i.attributes.position;if(l!==null){let u=Math.max(0,o.start),g=Math.min(l.count,o.start+o.count);for(let _=u,x=g;_<x;_++){let f=l.getX(_);Go.fromBufferAttribute(m,f),Sh(Go,f,c,r,e,t,this)}}else{let u=Math.max(0,o.start),g=Math.min(m.count,o.start+o.count);for(let _=u,x=g;_<x;_++)Go.fromBufferAttribute(m,_),Sh(Go,_,c,r,e,t,this)}}updateMorphTargets(){let t=this.geometry.morphAttributes,i=Object.keys(t);if(i.length>0){let r=t[i[0]];if(r!==void 0){this.morphTargetInfluences=[],this.morphTargetDictionary={};for(let s=0,o=r.length;s<o;s++){let a=r[s].name||String(s);this.morphTargetInfluences.push(0),this.morphTargetDictionary[a]=s}}}}};function Sh(n,e,t,i,r,s,o){let a=bc.distanceSqToPoint(n);if(a<t){let c=new V;bc.closestPointToPoint(n,c),c.applyMatrix4(i);let l=r.ray.origin.distanceTo(c);if(l<r.near||l>r.far)return;s.push({distance:l,distanceToRay:Math.sqrt(a),point:c,index:e,face:null,faceIndex:null,barycoord:null,object:o})}}var zs=class extends An{constructor(e=[],t=Ki,i,r,s,o,a,c,l,h){super(e,t,i,r,s,o,a,c,l,h),this.isCubeTexture=!0,this.flipY=!1}get images(){return this.image}set images(e){this.image=e}},dr=class extends An{constructor(e,t,i,r,s,o,a,c,l){super(e,t,i,r,s,o,a,c,l),this.isCanvasTexture=!0,this.needsUpdate=!0}};var Vi=class extends An{constructor(e,t,i=ii,r,s,o,a=hn,c=hn,l,h=di,m=1){if(h!==di&&h!==ji)throw new Error("THREE.DepthTexture: format must be either THREE.DepthFormat or THREE.DepthStencilFormat");let u={width:e,height:t,depth:m};super(u,r,s,o,a,c,h,i,l),this.isDepthTexture=!0,this.flipY=!1,this.generateMipmaps=!1,this.compareFunction=null}copy(e){return super.copy(e),this.source=new Kr(Object.assign({},e.image)),this.compareFunction=e.compareFunction,this}toJSON(e){let t=super.toJSON(e);return t.compareFunction=this.compareFunction,t}},fa=class extends Vi{constructor(e,t=ii,i=Ki,r,s,o=hn,a=hn,c,l=di){let h={width:e,height:e,depth:1},m=[h,h,h,h,h,h];super(e,e,t,i,r,s,o,a,c,l),this.image=m,this.isCubeDepthTexture=!0,this.isCubeTexture=!0}get images(){return this.image}set images(e){this.image=e}},Hs=class extends An{constructor(e=null){super(),this.sourceTexture=e,this.isExternalTexture=!0}copy(e){return super.copy(e),this.sourceTexture=e.sourceTexture,this}},Ke=class n extends Xt{constructor(e=1,t=1,i=1,r=1,s=1,o=1){super(),this.type="BoxGeometry",this.parameters={width:e,height:t,depth:i,widthSegments:r,heightSegments:s,depthSegments:o};let a=this;r=Math.floor(r),s=Math.floor(s),o=Math.floor(o);let c=[],l=[],h=[],m=[],u=0,g=0;_("z","y","x",-1,-1,i,t,e,o,s,0),_("z","y","x",1,-1,i,t,-e,o,s,1),_("x","z","y",1,1,e,i,t,r,o,2),_("x","z","y",1,-1,e,i,-t,r,o,3),_("x","y","z",1,-1,e,t,i,r,s,4),_("x","y","z",-1,-1,e,t,-i,r,s,5),this.setIndex(c),this.setAttribute("position",new ct(l,3)),this.setAttribute("normal",new ct(h,3)),this.setAttribute("uv",new ct(m,2));function _(x,f,d,S,P,w,T,A,D,p,M){let y=w/D,C=T/p,k=w/2,L=T/2,U=A/2,F=D+1,H=p+1,q=0,ee=0,J=new V;for(let ne=0;ne<H;ne++){let ce=ne*C-L;for(let pe=0;pe<F;pe++){let me=pe*y-k;J[x]=me*S,J[f]=ce*P,J[d]=U,l.push(J.x,J.y,J.z),J[x]=0,J[f]=0,J[d]=A>0?1:-1,h.push(J.x,J.y,J.z),m.push(pe/D),m.push(1-ne/p),q+=1}}for(let ne=0;ne<p;ne++)for(let ce=0;ce<D;ce++){let pe=u+ce+F*ne,me=u+ce+F*(ne+1),qe=u+(ce+1)+F*(ne+1),Le=u+(ce+1)+F*ne;c.push(pe,me,Le),c.push(me,qe,Le),ee+=6}a.addGroup(g,ee,M),g+=ee,u+=q}}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.width,e.height,e.depth,e.widthSegments,e.heightSegments,e.depthSegments)}};var pi=class n extends Xt{constructor(e=1,t=32,i=0,r=Math.PI*2){super(),this.type="CircleGeometry",this.parameters={radius:e,segments:t,thetaStart:i,thetaLength:r},t=Math.max(3,t);let s=[],o=[],a=[],c=[],l=new V,h=new He;o.push(0,0,0),a.push(0,0,1),c.push(.5,.5);for(let m=0,u=3;m<=t;m++,u+=3){let g=i+m/t*r;l.x=e*Math.cos(g),l.y=e*Math.sin(g),o.push(l.x,l.y,l.z),a.push(0,0,1),h.x=(o[u]/e+1)/2,h.y=(o[u+1]/e+1)/2,c.push(h.x,h.y)}for(let m=1;m<=t;m++)s.push(m,m+1,0);this.setIndex(s),this.setAttribute("position",new ct(o,3)),this.setAttribute("normal",new ct(a,3)),this.setAttribute("uv",new ct(c,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radius,e.segments,e.thetaStart,e.thetaLength)}},Dt=class n extends Xt{constructor(e=1,t=1,i=1,r=32,s=1,o=!1,a=0,c=Math.PI*2){super(),this.type="CylinderGeometry",this.parameters={radiusTop:e,radiusBottom:t,height:i,radialSegments:r,heightSegments:s,openEnded:o,thetaStart:a,thetaLength:c};let l=this;r=Math.floor(r),s=Math.floor(s);let h=[],m=[],u=[],g=[],_=0,x=[],f=i/2,d=0;S(),o===!1&&(e>0&&P(!0),t>0&&P(!1)),this.setIndex(h),this.setAttribute("position",new ct(m,3)),this.setAttribute("normal",new ct(u,3)),this.setAttribute("uv",new ct(g,2));function S(){let w=new V,T=new V,A=0,D=(t-e)/i;for(let p=0;p<=s;p++){let M=[],y=p/s,C=y*(t-e)+e;for(let k=0;k<=r;k++){let L=k/r,U=L*c+a,F=Math.sin(U),H=Math.cos(U);T.x=C*F,T.y=-y*i+f,T.z=C*H,m.push(T.x,T.y,T.z),w.set(F,D,H).normalize(),u.push(w.x,w.y,w.z),g.push(L,1-y),M.push(_++)}x.push(M)}for(let p=0;p<r;p++)for(let M=0;M<s;M++){let y=x[M][p],C=x[M+1][p],k=x[M+1][p+1],L=x[M][p+1];(e>0||M!==0)&&(h.push(y,C,L),A+=3),(t>0||M!==s-1)&&(h.push(C,k,L),A+=3)}l.addGroup(d,A,0),d+=A}function P(w){let T=_,A=new He,D=new V,p=0,M=w===!0?e:t,y=w===!0?1:-1;for(let k=1;k<=r;k++)m.push(0,f*y,0),u.push(0,y,0),g.push(.5,.5),_++;let C=_;for(let k=0;k<=r;k++){let U=k/r*c+a,F=Math.cos(U),H=Math.sin(U);D.x=M*H,D.y=f*y,D.z=M*F,m.push(D.x,D.y,D.z),u.push(0,y,0),A.x=F*.5+.5,A.y=H*.5*y+.5,g.push(A.x,A.y),_++}for(let k=0;k<r;k++){let L=T+k,U=C+k;w===!0?h.push(U,U+1,L):h.push(U+1,U,L),p+=3}l.addGroup(d,p,w===!0?1:2),d+=p}}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radiusTop,e.radiusBottom,e.height,e.radialSegments,e.heightSegments,e.openEnded,e.thetaStart,e.thetaLength)}},ns=class n extends Dt{constructor(e=1,t=1,i=32,r=1,s=!1,o=0,a=Math.PI*2){super(0,e,t,i,r,s,o,a),this.type="ConeGeometry",this.parameters={radius:e,height:t,radialSegments:i,heightSegments:r,openEnded:s,thetaStart:o,thetaLength:a}}static fromJSON(e){return new n(e.radius,e.height,e.radialSegments,e.heightSegments,e.openEnded,e.thetaStart,e.thetaLength)}},pa=class n extends Xt{constructor(e=[],t=[],i=1,r=0){super(),this.type="PolyhedronGeometry",this.parameters={vertices:e,indices:t,radius:i,detail:r};let s=[],o=[];a(r),l(i),h(),this.setAttribute("position",new ct(s,3)),this.setAttribute("normal",new ct(s.slice(),3)),this.setAttribute("uv",new ct(o,2)),r===0?this.computeVertexNormals():this.normalizeNormals();function a(S){let P=new V,w=new V,T=new V;for(let A=0;A<t.length;A+=3)g(t[A+0],P),g(t[A+1],w),g(t[A+2],T),c(P,w,T,S)}function c(S,P,w,T){let A=T+1,D=[];for(let p=0;p<=A;p++){D[p]=[];let M=S.clone().lerp(w,p/A),y=P.clone().lerp(w,p/A),C=A-p;for(let k=0;k<=C;k++)k===0&&p===A?D[p][k]=M:D[p][k]=M.clone().lerp(y,k/C)}for(let p=0;p<A;p++)for(let M=0;M<2*(A-p)-1;M++){let y=Math.floor(M/2);M%2===0?(u(D[p][y+1]),u(D[p+1][y]),u(D[p][y])):(u(D[p][y+1]),u(D[p+1][y+1]),u(D[p+1][y]))}}function l(S){let P=new V;for(let w=0;w<s.length;w+=3)P.x=s[w+0],P.y=s[w+1],P.z=s[w+2],P.normalize().multiplyScalar(S),s[w+0]=P.x,s[w+1]=P.y,s[w+2]=P.z}function h(){let S=new V;for(let P=0;P<s.length;P+=3){S.x=s[P+0],S.y=s[P+1],S.z=s[P+2];let w=f(S)/2/Math.PI+.5,T=d(S)/Math.PI+.5;o.push(w,1-T)}_(),m()}function m(){for(let S=0;S<o.length;S+=6){let P=o[S+0],w=o[S+2],T=o[S+4],A=Math.max(P,w,T),D=Math.min(P,w,T);A>.9&&D<.1&&(P<.2&&(o[S+0]+=1),w<.2&&(o[S+2]+=1),T<.2&&(o[S+4]+=1))}}function u(S){s.push(S.x,S.y,S.z)}function g(S,P){let w=S*3;P.x=e[w+0],P.y=e[w+1],P.z=e[w+2]}function _(){let S=new V,P=new V,w=new V,T=new V,A=new He,D=new He,p=new He;for(let M=0,y=0;M<s.length;M+=9,y+=6){S.set(s[M+0],s[M+1],s[M+2]),P.set(s[M+3],s[M+4],s[M+5]),w.set(s[M+6],s[M+7],s[M+8]),A.set(o[y+0],o[y+1]),D.set(o[y+2],o[y+3]),p.set(o[y+4],o[y+5]),T.copy(S).add(P).add(w).divideScalar(3);let C=f(T);x(A,y+0,S,C),x(D,y+2,P,C),x(p,y+4,w,C)}}function x(S,P,w,T){T<0&&S.x===1&&(o[P]=S.x-1),w.x===0&&w.z===0&&(o[P]=T/2/Math.PI+.5)}function f(S){return Math.atan2(S.z,-S.x)}function d(S){return Math.atan2(-S.y,Math.sqrt(S.x*S.x+S.z*S.z))}}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.vertices,e.indices,e.radius,e.detail)}},Vs=class n extends pa{constructor(e=1,t=0){let i=(1+Math.sqrt(5))/2,r=1/i,s=[-1,-1,-1,-1,-1,1,-1,1,-1,-1,1,1,1,-1,-1,1,-1,1,1,1,-1,1,1,1,0,-r,-i,0,-r,i,0,r,-i,0,r,i,-r,-i,0,-r,i,0,r,-i,0,r,i,0,-i,0,-r,i,0,-r,-i,0,r,i,0,r],o=[3,11,7,3,7,15,3,15,13,7,19,17,7,17,6,7,6,15,17,4,8,17,8,10,17,10,6,8,0,16,8,16,2,8,2,10,0,12,1,0,1,18,0,18,16,6,10,2,6,2,13,6,13,15,2,16,18,2,18,3,2,3,13,18,1,9,18,9,11,18,11,3,4,14,12,4,12,0,4,0,8,11,9,5,11,5,19,11,19,7,19,5,14,19,14,4,19,4,17,1,12,14,1,14,5,1,5,9];super(s,o,e,t),this.type="DodecahedronGeometry",this.parameters={radius:e,detail:t}}static fromJSON(e){return new n(e.radius,e.detail)}},Wo=new V,Xo=new V,pc=new V,qo=new Ei,Gs=class extends Xt{constructor(e=null,t=1){if(super(),this.type="EdgesGeometry",this.parameters={geometry:e,thresholdAngle:t},e!==null){let r=Math.pow(10,4),s=Math.cos(Wr*t),o=e.getIndex(),a=e.getAttribute("position"),c=o?o.count:a.count,l=[0,0,0],h=["a","b","c"],m=new Array(3),u={},g=[];for(let _=0;_<c;_+=3){o?(l[0]=o.getX(_),l[1]=o.getX(_+1),l[2]=o.getX(_+2)):(l[0]=_,l[1]=_+1,l[2]=_+2);let{a:x,b:f,c:d}=qo;if(x.fromBufferAttribute(a,l[0]),f.fromBufferAttribute(a,l[1]),d.fromBufferAttribute(a,l[2]),qo.getNormal(pc),m[0]=`${Math.round(x.x*r)},${Math.round(x.y*r)},${Math.round(x.z*r)}`,m[1]=`${Math.round(f.x*r)},${Math.round(f.y*r)},${Math.round(f.z*r)}`,m[2]=`${Math.round(d.x*r)},${Math.round(d.y*r)},${Math.round(d.z*r)}`,!(m[0]===m[1]||m[1]===m[2]||m[2]===m[0]))for(let S=0;S<3;S++){let P=(S+1)%3,w=m[S],T=m[P],A=qo[h[S]],D=qo[h[P]],p=`${w}_${T}`,M=`${T}_${w}`;M in u&&u[M]?(pc.dot(u[M].normal)<=s&&(g.push(A.x,A.y,A.z),g.push(D.x,D.y,D.z)),u[M]=null):p in u||(u[p]={index0:l[S],index1:l[P],normal:pc.clone()})}}for(let _ in u)if(u[_]){let{index0:x,index1:f}=u[_];Wo.fromBufferAttribute(a,x),Xo.fromBufferAttribute(a,f),g.push(Wo.x,Wo.y,Wo.z),g.push(Xo.x,Xo.y,Xo.z)}this.setAttribute("position",new ct(g,3))}}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}};var Gi=class n extends Xt{constructor(e=1,t=1,i=1,r=1){super(),this.type="PlaneGeometry",this.parameters={width:e,height:t,widthSegments:i,heightSegments:r};let s=e/2,o=t/2,a=Math.floor(i),c=Math.floor(r),l=a+1,h=c+1,m=e/a,u=t/c,g=[],_=[],x=[],f=[];for(let d=0;d<h;d++){let S=d*u-o;for(let P=0;P<l;P++){let w=P*m-s;_.push(w,-S,0),x.push(0,0,1),f.push(P/a),f.push(1-d/c)}}for(let d=0;d<c;d++)for(let S=0;S<a;S++){let P=S+l*d,w=S+l*(d+1),T=S+1+l*(d+1),A=S+1+l*d;g.push(P,w,A),g.push(w,T,A)}this.setIndex(g),this.setAttribute("position",new ct(_,3)),this.setAttribute("normal",new ct(x,3)),this.setAttribute("uv",new ct(f,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.width,e.height,e.widthSegments,e.heightSegments)}},Ws=class n extends Xt{constructor(e=.5,t=1,i=32,r=1,s=0,o=Math.PI*2){super(),this.type="RingGeometry",this.parameters={innerRadius:e,outerRadius:t,thetaSegments:i,phiSegments:r,thetaStart:s,thetaLength:o},i=Math.max(3,i),r=Math.max(1,r);let a=[],c=[],l=[],h=[],m=e,u=(t-e)/r,g=new V,_=new He;for(let x=0;x<=r;x++){for(let f=0;f<=i;f++){let d=s+f/i*o;g.x=m*Math.cos(d),g.y=m*Math.sin(d),c.push(g.x,g.y,g.z),l.push(0,0,1),_.x=(g.x/t+1)/2,_.y=(g.y/t+1)/2,h.push(_.x,_.y)}m+=u}for(let x=0;x<r;x++){let f=x*(i+1);for(let d=0;d<i;d++){let S=d+f,P=S,w=S+i+1,T=S+i+2,A=S+1;a.push(P,w,A),a.push(w,T,A)}}this.setIndex(a),this.setAttribute("position",new ct(c,3)),this.setAttribute("normal",new ct(l,3)),this.setAttribute("uv",new ct(h,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.innerRadius,e.outerRadius,e.thetaSegments,e.phiSegments,e.thetaStart,e.thetaLength)}};var Jt=class n extends Xt{constructor(e=1,t=32,i=16,r=0,s=Math.PI*2,o=0,a=Math.PI){super(),this.type="SphereGeometry",this.parameters={radius:e,widthSegments:t,heightSegments:i,phiStart:r,phiLength:s,thetaStart:o,thetaLength:a},t=Math.max(3,Math.floor(t)),i=Math.max(2,Math.floor(i));let c=Math.min(o+a,Math.PI),l=0,h=[],m=new V,u=new V,g=[],_=[],x=[],f=[];for(let d=0;d<=i;d++){let S=[],P=d/i,w=o+P*a,T=e*Math.cos(w),A=Math.sqrt(e*e-T*T),D=0;d===0&&o===0?D=.5/t:d===i&&c===Math.PI&&(D=-.5/t);for(let p=0;p<=t;p++){let M=p/t,y=r+M*s;m.x=-A*Math.cos(y),m.y=T,m.z=A*Math.sin(y),_.push(m.x,m.y,m.z),u.copy(m).normalize(),x.push(u.x,u.y,u.z),f.push(M+D,1-P),S.push(l++)}h.push(S)}for(let d=0;d<i;d++)for(let S=0;S<t;S++){let P=h[d][S+1],w=h[d][S],T=h[d+1][S],A=h[d+1][S+1];(d!==0||o>0)&&g.push(P,w,A),(d!==i-1||c<Math.PI)&&g.push(w,T,A)}this.setIndex(g),this.setAttribute("position",new ct(_,3)),this.setAttribute("normal",new ct(x,3)),this.setAttribute("uv",new ct(f,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radius,e.widthSegments,e.heightSegments,e.phiStart,e.phiLength,e.thetaStart,e.thetaLength)}};var fr=class n extends Xt{constructor(e=1,t=.4,i=12,r=48,s=Math.PI*2,o=0,a=Math.PI*2){super(),this.type="TorusGeometry",this.parameters={radius:e,tube:t,radialSegments:i,tubularSegments:r,arc:s,thetaStart:o,thetaLength:a},i=Math.floor(i),r=Math.floor(r);let c=[],l=[],h=[],m=[],u=new V,g=new V,_=new V;for(let x=0;x<=i;x++){let f=o+x/i*a;for(let d=0;d<=r;d++){let S=d/r*s;g.x=(e+t*Math.cos(f))*Math.cos(S),g.y=(e+t*Math.cos(f))*Math.sin(S),g.z=t*Math.sin(f),l.push(g.x,g.y,g.z),u.x=e*Math.cos(S),u.y=e*Math.sin(S),_.subVectors(g,u).normalize(),h.push(_.x,_.y,_.z),m.push(d/r),m.push(x/i)}}for(let x=1;x<=i;x++)for(let f=1;f<=r;f++){let d=(r+1)*x+f-1,S=(r+1)*(x-1)+f-1,P=(r+1)*(x-1)+f,w=(r+1)*x+f;c.push(d,S,w),c.push(S,P,w)}this.setIndex(c),this.setAttribute("position",new ct(l,3)),this.setAttribute("normal",new ct(h,3)),this.setAttribute("uv",new ct(m,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radius,e.tube,e.radialSegments,e.tubularSegments,e.arc,e.thetaStart,e.thetaLength)}};function gr(n){let e={};for(let t in n){e[t]={};for(let i in n[t]){let r=n[t][i];if(wh(r))r.isRenderTargetTexture?(Ge("UniformsUtils: Textures of render targets cannot be cloned via cloneUniforms() or mergeUniforms()."),e[t][i]=null):e[t][i]=r.clone();else if(Array.isArray(r))if(wh(r[0])){let s=[];for(let o=0,a=r.length;o<a;o++)s[o]=r[o].clone();e[t][i]=s}else e[t][i]=r.slice();else e[t][i]=r}}return e}function Sn(n){let e={};for(let t=0;t<n.length;t++){let i=gr(n[t]);for(let r in i)e[r]=i[r]}return e}function wh(n){return n&&(n.isColor||n.isMatrix3||n.isMatrix4||n.isVector2||n.isVector3||n.isVector4||n.isTexture||n.isQuaternion)}function Zp(n){let e=[];for(let t=0;t<n.length;t++)e.push(n[t].clone());return e}function jc(n){let e=n.getRenderTarget();return e===null?n.outputColorSpace:e.isXRRenderTarget===!0?e.texture.colorSpace:lt.workingColorSpace}var gd={clone:gr,merge:Sn},Kp=`void main() {
	gl_Position = projectionMatrix * modelViewMatrix * vec4( position, 1.0 );
}`,Jp=`void main() {
	gl_FragColor = vec4( 1.0, 0.0, 0.0, 1.0 );
}`,On=class extends fi{constructor(e){super(),this.isShaderMaterial=!0,this.type="ShaderMaterial",this.defines={},this.uniforms={},this.uniformsGroups=[],this.vertexShader=Kp,this.fragmentShader=Jp,this.linewidth=1,this.wireframe=!1,this.wireframeLinewidth=1,this.fog=!1,this.lights=!1,this.clipping=!1,this.forceSinglePass=!0,this.extensions={clipCullDistance:!1,multiDraw:!1},this.defaultAttributeValues={color:[1,1,1],uv:[0,0],uv1:[0,0]},this.index0AttributeName=void 0,this.uniformsNeedUpdate=!1,this.glslVersion=null,e!==void 0&&this.setValues(e)}copy(e){return super.copy(e),this.fragmentShader=e.fragmentShader,this.vertexShader=e.vertexShader,this.uniforms=gr(e.uniforms),this.uniformsGroups=Zp(e.uniformsGroups),this.defines=Object.assign({},e.defines),this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this.fog=e.fog,this.lights=e.lights,this.clipping=e.clipping,this.extensions=Object.assign({},e.extensions),this.glslVersion=e.glslVersion,this.defaultAttributeValues=Object.assign({},e.defaultAttributeValues),this.index0AttributeName=e.index0AttributeName,this.uniformsNeedUpdate=e.uniformsNeedUpdate,this}toJSON(e){let t=super.toJSON(e);t.glslVersion=this.glslVersion,t.uniforms={};for(let r in this.uniforms){let o=this.uniforms[r].value;o&&o.isTexture?t.uniforms[r]={type:"t",value:o.toJSON(e).uuid}:o&&o.isColor?t.uniforms[r]={type:"c",value:o.getHex()}:o&&o.isVector2?t.uniforms[r]={type:"v2",value:o.toArray()}:o&&o.isVector3?t.uniforms[r]={type:"v3",value:o.toArray()}:o&&o.isVector4?t.uniforms[r]={type:"v4",value:o.toArray()}:o&&o.isMatrix3?t.uniforms[r]={type:"m3",value:o.toArray()}:o&&o.isMatrix4?t.uniforms[r]={type:"m4",value:o.toArray()}:t.uniforms[r]={value:o}}Object.keys(this.defines).length>0&&(t.defines=this.defines),t.vertexShader=this.vertexShader,t.fragmentShader=this.fragmentShader,t.lights=this.lights,t.clipping=this.clipping;let i={};for(let r in this.extensions)this.extensions[r]===!0&&(i[r]=!0);return Object.keys(i).length>0&&(t.extensions=i),t}fromJSON(e,t){if(super.fromJSON(e,t),e.uniforms!==void 0)for(let i in e.uniforms){let r=e.uniforms[i];switch(this.uniforms[i]={},r.type){case"t":this.uniforms[i].value=t[r.value]||null;break;case"c":this.uniforms[i].value=new ze().setHex(r.value);break;case"v2":this.uniforms[i].value=new He().fromArray(r.value);break;case"v3":this.uniforms[i].value=new V().fromArray(r.value);break;case"v4":this.uniforms[i].value=new Ot().fromArray(r.value);break;case"m3":this.uniforms[i].value=new Ze().fromArray(r.value);break;case"m4":this.uniforms[i].value=new It().fromArray(r.value);break;default:this.uniforms[i].value=r.value}}if(e.defines!==void 0&&(this.defines=e.defines),e.vertexShader!==void 0&&(this.vertexShader=e.vertexShader),e.fragmentShader!==void 0&&(this.fragmentShader=e.fragmentShader),e.glslVersion!==void 0&&(this.glslVersion=e.glslVersion),e.extensions!==void 0)for(let i in e.extensions)this.extensions[i]=e.extensions[i];return e.lights!==void 0&&(this.lights=e.lights),e.clipping!==void 0&&(this.clipping=e.clipping),this}},ma=class extends On{constructor(e){super(e),this.isRawShaderMaterial=!0,this.type="RawShaderMaterial"}},St=class extends fi{constructor(e){super(),this.isMeshStandardMaterial=!0,this.type="MeshStandardMaterial",this.defines={STANDARD:""},this.color=new ze(16777215),this.roughness=1,this.metalness=0,this.map=null,this.lightMap=null,this.lightMapIntensity=1,this.aoMap=null,this.aoMapIntensity=1,this.emissive=new ze(0),this.emissiveIntensity=1,this.emissiveMap=null,this.bumpMap=null,this.bumpScale=1,this.normalMap=null,this.normalMapType=gl,this.normalScale=new He(1,1),this.displacementMap=null,this.displacementScale=1,this.displacementBias=0,this.roughnessMap=null,this.metalnessMap=null,this.alphaMap=null,this.envMap=null,this.envMapRotation=new Ai,this.envMapIntensity=1,this.wireframe=!1,this.wireframeLinewidth=1,this.wireframeLinecap="round",this.wireframeLinejoin="round",this.flatShading=!1,this.fog=!0,this.setValues(e)}copy(e){return super.copy(e),this.defines={STANDARD:""},this.color.copy(e.color),this.roughness=e.roughness,this.metalness=e.metalness,this.map=e.map,this.lightMap=e.lightMap,this.lightMapIntensity=e.lightMapIntensity,this.aoMap=e.aoMap,this.aoMapIntensity=e.aoMapIntensity,this.emissive.copy(e.emissive),this.emissiveMap=e.emissiveMap,this.emissiveIntensity=e.emissiveIntensity,this.bumpMap=e.bumpMap,this.bumpScale=e.bumpScale,this.normalMap=e.normalMap,this.normalMapType=e.normalMapType,this.normalScale.copy(e.normalScale),this.displacementMap=e.displacementMap,this.displacementScale=e.displacementScale,this.displacementBias=e.displacementBias,this.roughnessMap=e.roughnessMap,this.metalnessMap=e.metalnessMap,this.alphaMap=e.alphaMap,this.envMap=e.envMap,this.envMapRotation.copy(e.envMapRotation),this.envMapIntensity=e.envMapIntensity,this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this.wireframeLinecap=e.wireframeLinecap,this.wireframeLinejoin=e.wireframeLinejoin,this.flatShading=e.flatShading,this.fog=e.fog,this}};var ga=class extends fi{constructor(e){super(),this.isMeshDepthMaterial=!0,this.type="MeshDepthMaterial",this.depthPacking=td,this.map=null,this.alphaMap=null,this.displacementMap=null,this.displacementScale=1,this.displacementBias=0,this.wireframe=!1,this.wireframeLinewidth=1,this.setValues(e)}copy(e){return super.copy(e),this.depthPacking=e.depthPacking,this.map=e.map,this.alphaMap=e.alphaMap,this.displacementMap=e.displacementMap,this.displacementScale=e.displacementScale,this.displacementBias=e.displacementBias,this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this}},xa=class extends fi{constructor(e){super(),this.isMeshDistanceMaterial=!0,this.type="MeshDistanceMaterial",this.map=null,this.alphaMap=null,this.displacementMap=null,this.displacementScale=1,this.displacementBias=0,this.setValues(e)}copy(e){return super.copy(e),this.map=e.map,this.alphaMap=e.alphaMap,this.displacementMap=e.displacementMap,this.displacementScale=e.displacementScale,this.displacementBias=e.displacementBias,this}};function zr(n,e){return!n||n.constructor===e?n:typeof e.BYTES_PER_ELEMENT=="number"?new e(n):Array.prototype.slice.call(n)}function mc(n){return n!==void 0&&n.inTangents!==void 0&&n.outTangents!==void 0}var Wi=class{constructor(e,t,i,r){this.parameterPositions=e,this._cachedIndex=0,this.resultBuffer=r!==void 0?r:new t.constructor(i),this.sampleValues=t,this.valueSize=i,this.settings=null,this.DefaultSettings_={}}evaluate(e){let t=this.parameterPositions,i=this._cachedIndex,r=t[i],s=t[i-1];n:{e:{let o;t:{i:if(!(e<r)){for(let a=i+2;;){if(r===void 0){if(e<s)break i;return i=t.length,this._cachedIndex=i,this.copySampleValue_(i-1)}if(i===a)break;if(s=r,r=t[++i],e<r)break e}o=t.length;break t}if(!(e>=s)){let a=t[1];e<a&&(i=2,s=a);for(let c=i-2;;){if(s===void 0)return this._cachedIndex=0,this.copySampleValue_(0);if(i===c)break;if(r=s,s=t[--i-1],e>=s)break e}o=i,i=0;break t}break n}for(;i<o;){let a=i+o>>>1;e<t[a]?o=a:i=a+1}if(r=t[i],s=t[i-1],s===void 0)return this._cachedIndex=0,this.copySampleValue_(0);if(r===void 0)return i=t.length,this._cachedIndex=i,this.copySampleValue_(i-1)}this._cachedIndex=i,this.intervalChanged_(i,s,r)}return this.interpolate_(i,s,e,r)}getSettings_(){return this.settings||this.DefaultSettings_}copySampleValue_(e){let t=this.resultBuffer,i=this.sampleValues,r=this.valueSize,s=e*r;for(let o=0;o!==r;++o)t[o]=i[s+o];return t}interpolate_(){throw new Error("THREE.Interpolant: Call to abstract method.")}intervalChanged_(){}},va=class extends Wi{constructor(e,t,i,r){super(e,t,i,r),this._weightPrev=-0,this._offsetPrev=-0,this._weightNext=-0,this._offsetNext=-0,this.DefaultSettings_={endingStart:vc,endingEnd:vc}}intervalChanged_(e,t,i){let r=this.parameterPositions,s=e-2,o=e+1,a=r[s],c=r[o];if(a===void 0)switch(this.getSettings_().endingStart){case _c:s=e,a=2*t-i;break;case yc:s=r.length-2,a=t+r[s]-r[s+1];break;default:s=e,a=i}if(c===void 0)switch(this.getSettings_().endingEnd){case _c:o=e,c=2*i-t;break;case yc:o=1,c=i+r[1]-r[0];break;default:o=e-1,c=t}let l=(i-t)*.5,h=this.valueSize;this._weightPrev=l/(t-a),this._weightNext=l/(c-i),this._offsetPrev=s*h,this._offsetNext=o*h}interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,c=e*a,l=c-a,h=this._offsetPrev,m=this._offsetNext,u=this._weightPrev,g=this._weightNext,_=(i-t)/(r-t),x=_*_,f=x*_,d=-u*f+2*u*x-u*_,S=(1+u)*f+(-1.5-2*u)*x+(-.5+u)*_+1,P=(-1-g)*f+(1.5+g)*x+.5*_,w=g*f-g*x;for(let T=0;T!==a;++T)s[T]=d*o[h+T]+S*o[l+T]+P*o[c+T]+w*o[m+T];return s}},_a=class extends Wi{constructor(e,t,i,r){super(e,t,i,r)}interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,c=e*a,l=c-a,h=(i-t)/(r-t),m=1-h;for(let u=0;u!==a;++u)s[u]=o[l+u]*m+o[c+u]*h;return s}},ya=class extends Wi{constructor(e,t,i,r){super(e,t,i,r)}interpolate_(e){return this.copySampleValue_(e-1)}},ba=class extends Wi{interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,c=e*a,l=c-a,h=this.inTangents,m=this.outTangents;if(!h||!m){let _=(i-t)/(r-t),x=1-_;for(let f=0;f!==a;++f)s[f]=o[l+f]*x+o[c+f]*_;return s}let u=a*2,g=e-1;for(let _=0;_!==a;++_){let x=o[l+_],f=o[c+_],d=g*u+_*2,S=m[d],P=m[d+1],w=e*u+_*2,T=h[w],A=h[w+1],D=Qp(i,t,S,T,r);s[_]=xd(D,x,P,A,f)}return s}};function xd(n,e,t,i,r){let s=1-n;return s*s*s*e+3*s*s*n*t+3*s*n*n*i+n*n*n*r}function jp(n,e,t,i,r){let s=1-n;return 3*s*s*(t-e)+6*s*n*(i-t)+3*n*n*(r-i)}function Qp(n,e,t,i,r){let s=(n-e)/(r-e);for(let o=0;o<8;o++){let a=xd(s,e,t,i,r)-n;if(Math.abs(a)<1e-10)break;let c=jp(s,e,t,i,r);if(Math.abs(c)<1e-10)break;s=Math.max(0,Math.min(1,s-a/c))}return s}var Bn=class{constructor(e,t,i,r){if(e===void 0)throw new Error("THREE.KeyframeTrack: track name is undefined");if(t===void 0||t.length===0)throw new Error("THREE.KeyframeTrack: no keyframes in track named "+e);this.name=e,this.times=zr(t,this.TimeBufferType),this.values=zr(i,this.ValueBufferType),this.setInterpolation(r||this.DefaultInterpolation)}static toJSON(e){let t=e.constructor,i;if(t.toJSON!==this.toJSON)i=t.toJSON(e);else{i={name:e.name,times:zr(e.times,Array),values:zr(e.values,Array)};let r=e.getInterpolation();r!==e.DefaultInterpolation&&(i.interpolation=r),mc(e.settings)&&(i.settings={inTangents:zr(e.settings.inTangents,Array),outTangents:zr(e.settings.outTangents,Array)})}return i.type=e.ValueTypeName,i}InterpolantFactoryMethodDiscrete(e){return new ya(this.times,this.values,this.getValueSize(),e)}InterpolantFactoryMethodLinear(e){return new _a(this.times,this.values,this.getValueSize(),e)}InterpolantFactoryMethodSmooth(e){return new va(this.times,this.values,this.getValueSize(),e)}InterpolantFactoryMethodBezier(e){let t=new ba(this.times,this.values,this.getValueSize(),e);return this.settings&&(t.inTangents=this.settings.inTangents,t.outTangents=this.settings.outTangents),t}setInterpolation(e){let t;switch(e){case Is:t=this.InterpolantFactoryMethodDiscrete;break;case sa:t=this.InterpolantFactoryMethodLinear;break;case Zo:t=this.InterpolantFactoryMethodSmooth;break;case xc:t=this.InterpolantFactoryMethodBezier;break}if(t===void 0){let i="unsupported interpolation for "+this.ValueTypeName+" keyframe track named "+this.name;if(this.createInterpolant===void 0)if(e!==this.DefaultInterpolation)this.setInterpolation(this.DefaultInterpolation);else throw new Error(i);return Ge("KeyframeTrack:",i),this}return this.createInterpolant=t,this}getInterpolation(){switch(this.createInterpolant){case this.InterpolantFactoryMethodDiscrete:return Is;case this.InterpolantFactoryMethodLinear:return sa;case this.InterpolantFactoryMethodSmooth:return Zo;case this.InterpolantFactoryMethodBezier:return xc}}getValueSize(){return this.values.length/this.times.length}shift(e){if(e!==0){let t=this.times;for(let i=0,r=t.length;i!==r;++i)t[i]+=e}return this}scale(e){if(e!==1){let t=this.times;for(let i=0,r=t.length;i!==r;++i)t[i]*=e;mc(this.settings)&&(Eh(this.settings.inTangents,e),Eh(this.settings.outTangents,e))}return this}trim(e,t){let i=this.times,r=i.length,s=0,o=r-1;for(;s!==r&&i[s]<e;)++s;for(;o!==-1&&i[o]>t;)--o;if(++o,s!==0||o!==r){s>=o&&(o=Math.max(o,1),s=o-1);let a=this.getValueSize();this.times=i.slice(s,o),this.values=this.values.slice(s*a,o*a)}return this}validate(){let e=!0,t=this.getValueSize();t-Math.floor(t)!==0&&(Xe("KeyframeTrack: Invalid value size in track.",this),e=!1);let i=this.times,r=this.values,s=i.length;s===0&&(Xe("KeyframeTrack: Track is empty.",this),e=!1);let o=null;for(let a=0;a!==s;a++){let c=i[a];if(typeof c=="number"&&isNaN(c)){Xe("KeyframeTrack: Time is not a valid number.",this,a,c),e=!1;break}if(o!==null&&o>c){Xe("KeyframeTrack: Out of order keys.",this,a,c,o),e=!1;break}o=c}if(r!==void 0&&pp(r))for(let a=0,c=r.length;a!==c;++a){let l=r[a];if(isNaN(l)){Xe("KeyframeTrack: Value is not a valid number.",this,a,l),e=!1;break}}return e}optimize(){let e=this.times.slice(),t=this.values.slice(),i=this.getValueSize(),r=this.getInterpolation()===Zo,s=e.length-1,o=1;for(let a=1;a<s;++a){let c=!1,l=e[a],h=e[a+1];if(l!==h&&(a!==1||l!==e[0]))if(r)c=!0;else{let m=a*i,u=m-i,g=m+i;for(let _=0;_!==i;++_){let x=t[m+_];if(x!==t[u+_]||x!==t[g+_]){c=!0;break}}}if(c){if(a!==o){e[o]=e[a];let m=a*i,u=o*i;for(let g=0;g!==i;++g)t[u+g]=t[m+g]}++o}}if(s>0){e[o]=e[s];for(let a=s*i,c=o*i,l=0;l!==i;++l)t[c+l]=t[a+l];++o}return o!==e.length?(this.times=e.slice(0,o),this.values=t.slice(0,o*i)):(this.times=e,this.values=t),this}clone(){let e=this.times.slice(),t=this.values.slice(),i=this.constructor,r=new i(this.name,e,t);return r.createInterpolant=this.createInterpolant,mc(this.settings)&&(r.settings={inTangents:this.settings.inTangents.slice(),outTangents:this.settings.outTangents.slice()}),r}};function Eh(n,e){for(let t=0,i=n.length;t!==i;t+=2)n[t]*=e}Bn.prototype.ValueTypeName="";Bn.prototype.TimeBufferType=Float32Array;Bn.prototype.ValueBufferType=Float32Array;Bn.prototype.DefaultInterpolation=sa;var Xi=class extends Bn{constructor(e,t,i){super(e,t,i)}};Xi.prototype.ValueTypeName="bool";Xi.prototype.ValueBufferType=Array;Xi.prototype.DefaultInterpolation=Is;Xi.prototype.InterpolantFactoryMethodLinear=void 0;Xi.prototype.InterpolantFactoryMethodSmooth=void 0;var Ma=class extends Bn{constructor(e,t,i,r){super(e,t,i,r)}};Ma.prototype.ValueTypeName="color";var Sa=class extends Bn{constructor(e,t,i,r){super(e,t,i,r)}};Sa.prototype.ValueTypeName="number";var wa=class extends Wi{constructor(e,t,i,r){super(e,t,i,r)}interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,c=(i-t)/(r-t),l=e*a;for(let h=l+a;l!==h;l+=4)Fn.slerpFlat(s,0,o,l-a,o,l,c);return s}},Xs=class extends Bn{constructor(e,t,i,r){super(e,t,i,r)}InterpolantFactoryMethodLinear(e){return new wa(this.times,this.values,this.getValueSize(),e)}};Xs.prototype.ValueTypeName="quaternion";Xs.prototype.InterpolantFactoryMethodSmooth=void 0;var qi=class extends Bn{constructor(e,t,i){super(e,t,i)}};qi.prototype.ValueTypeName="string";qi.prototype.ValueBufferType=Array;qi.prototype.DefaultInterpolation=Is;qi.prototype.InterpolantFactoryMethodLinear=void 0;qi.prototype.InterpolantFactoryMethodSmooth=void 0;var Ea=class extends Bn{constructor(e,t,i,r){super(e,t,i,r)}};Ea.prototype.ValueTypeName="vector";var Ta=class{constructor(e,t,i){let r=this,s=!1,o=0,a=0,c,l=[];this.onStart=void 0,this.onLoad=e,this.onProgress=t,this.onError=i,this._abortController=null,this.itemStart=function(h){a++,s===!1&&r.onStart!==void 0&&r.onStart(h,o,a),s=!0},this.itemEnd=function(h){o++,r.onProgress!==void 0&&r.onProgress(h,o,a),o===a&&(s=!1,r.onLoad!==void 0&&r.onLoad())},this.itemError=function(h){r.onError!==void 0&&r.onError(h)},this.resolveURL=function(h){return h=h.normalize("NFC"),c?c(h):h},this.setURLModifier=function(h){return c=h,this},this.addHandler=function(h,m){return l.push(h,m),this},this.removeHandler=function(h){let m=l.indexOf(h);return m!==-1&&l.splice(m,2),this},this.getHandler=function(h){for(let m=0,u=l.length;m<u;m+=2){let g=l[m],_=l[m+1];if(g.global&&(g.lastIndex=0),g.test(h))return _}return null},this.abort=function(){return this.abortController.abort(),this._abortController=null,this}}get abortController(){return this._abortController||(this._abortController=new AbortController),this._abortController}},vd=new Ta,Aa=class{constructor(e){this.manager=e!==void 0?e:vd,this.crossOrigin="anonymous",this.withCredentials=!1,this.path="",this.resourcePath="",this.requestHeader={},typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("observe",{detail:this}))}load(){}loadAsync(e,t){let i=this;return new Promise(function(r,s){i.load(e,r,t,s)})}parse(){}setCrossOrigin(e){return this.crossOrigin=e,this}setWithCredentials(e){return this.withCredentials=e,this}setPath(e){return this.path=e,this}setResourcePath(e){return this.resourcePath=e,this}setRequestHeader(e){return this.requestHeader=e,this}abort(){return this}};Aa.DEFAULT_MATERIAL_NAME="__DEFAULT";var is=class extends dn{constructor(e,t=1){super(),this.isLight=!0,this.type="Light",this.color=new ze(e),this.intensity=t}copy(e,t){return super.copy(e,t),this.color.copy(e.color),this.intensity=e.intensity,this}toJSON(e){let t=super.toJSON(e);return t.object.color=this.color.getHex(),t.object.intensity=this.intensity,t}},qs=class extends is{constructor(e,t,i){super(e,i),this.isHemisphereLight=!0,this.type="HemisphereLight",this.position.copy(dn.DEFAULT_UP),this.updateMatrix(),this.groundColor=new ze(t)}copy(e,t){return super.copy(e,t),this.groundColor.copy(e.groundColor),this}toJSON(e){let t=super.toJSON(e);return t.object.groundColor=this.groundColor.getHex(),t}},gc=new It,Th=new V,Ah=new V,$s=class{constructor(e){this.camera=e,this.intensity=1,this.bias=0,this.biasNode=null,this.normalBias=0,this.radius=1,this.blurSamples=8,this.mapSize=new He(512,512),this.mapType=Ln,this.map=null,this.mapPass=null,this.matrix=new It,this.autoUpdate=!0,this.needsUpdate=!1,this._frustum=new Qr,this._frameExtents=new He(1,1),this._viewportCount=1,this._viewports=[new Ot(0,0,1,1)]}getViewportCount(){return this._viewportCount}getCamera(){return this.camera}getFrustum(){return this._frustum}updateMatrices(e){let t=this.camera;Th.setFromMatrixPosition(e.matrixWorld),t.position.copy(Th),Ah.setFromMatrixPosition(e.target.matrixWorld),t.lookAt(Ah),t.updateMatrixWorld(),this._updateMatrix(t,this.matrix,this._frustum)}_updateMatrix(e,t,i,r){gc.multiplyMatrices(e.projectionMatrix,e.matrixWorldInverse),i.setFromProjectionMatrix(gc,e.coordinateSystem,e.reversedDepth);let s=this._frameExtents,o=r?r.z/s.x:1,a=r?r.w/s.y:1,c=r?r.x/s.x:0,l=r?r.y/s.y:0;e.coordinateSystem===$r||e.reversedDepth?t.set(.5*o,0,0,.5*o+c,0,.5*a,0,.5*a+l,0,0,1,0,0,0,0,1):t.set(.5*o,0,0,.5*o+c,0,.5*a,0,.5*a+l,0,0,.5,.5,0,0,0,1),t.multiply(gc)}getViewport(e){return this._viewports[e]}getFrameExtents(){return this._frameExtents}dispose(){this.map&&this.map.dispose(),this.mapPass&&this.mapPass.dispose()}copy(e){return this.camera=e.camera.clone(),this.intensity=e.intensity,this.bias=e.bias,this.radius=e.radius,this.autoUpdate=e.autoUpdate,this.needsUpdate=e.needsUpdate,this.normalBias=e.normalBias,this.blurSamples=e.blurSamples,this.mapSize.copy(e.mapSize),this.biasNode=e.biasNode,this}clone(){return new this.constructor().copy(this)}toJSON(){let e={};return e.intensity=this.intensity,e.bias=this.bias,e.normalBias=this.normalBias,e.radius=this.radius,e.blurSamples=this.blurSamples,e.mapSize=this.mapSize.toArray(),e.camera=this.camera.toJSON(!1).object,delete e.camera.matrix,e}},$o=new V,Yo=new Fn,ui=new V,Ys=class extends dn{constructor(){super(),this.isCamera=!0,this.type="Camera",this.matrixWorldInverse=new It,this.projectionMatrix=new It,this.projectionMatrixInverse=new It,this.coordinateSystem=ei,this._reversedDepth=!1}get reversedDepth(){return this._reversedDepth}copy(e,t){return super.copy(e,t),this.matrixWorldInverse.copy(e.matrixWorldInverse),this.projectionMatrix.copy(e.projectionMatrix),this.projectionMatrixInverse.copy(e.projectionMatrixInverse),this.coordinateSystem=e.coordinateSystem,this}getWorldDirection(e){return super.getWorldDirection(e).negate()}updateMatrixWorld(e){super.updateMatrixWorld(e),this.matrixWorld.decompose($o,Yo,ui),ui.x===1&&ui.y===1&&ui.z===1?this.matrixWorldInverse.copy(this.matrixWorld).invert():this.matrixWorldInverse.compose($o,Yo,ui.set(1,1,1)).invert()}updateWorldMatrix(e,t,i=!1){super.updateWorldMatrix(e,t,i),this.matrixWorld.decompose($o,Yo,ui),ui.x===1&&ui.y===1&&ui.z===1?this.matrixWorldInverse.copy(this.matrixWorld).invert():this.matrixWorldInverse.compose($o,Yo,ui.set(1,1,1)).invert()}clone(){return new this.constructor().copy(this)}},zi=new V,Rh=new He,Ch=new He,mn=class extends Ys{constructor(e=50,t=1,i=.1,r=2e3){super(),this.isPerspectiveCamera=!0,this.type="PerspectiveCamera",this.fov=e,this.zoom=1,this.near=i,this.far=r,this.focus=10,this.aspect=t,this.view=null,this.filmGauge=35,this.filmOffset=0,this.updateProjectionMatrix()}copy(e,t){return super.copy(e,t),this.fov=e.fov,this.zoom=e.zoom,this.near=e.near,this.far=e.far,this.focus=e.focus,this.aspect=e.aspect,this.view=e.view===null?null:Object.assign({},e.view),this.filmGauge=e.filmGauge,this.filmOffset=e.filmOffset,this}setFocalLength(e){let t=.5*this.getFilmHeight()/e;this.fov=Zr*2*Math.atan(t),this.updateProjectionMatrix()}getFocalLength(){let e=Math.tan(Wr*.5*this.fov);return .5*this.getFilmHeight()/e}getEffectiveFOV(){return Zr*2*Math.atan(Math.tan(Wr*.5*this.fov)/this.zoom)}getFilmWidth(){return this.filmGauge*Math.min(this.aspect,1)}getFilmHeight(){return this.filmGauge/Math.max(this.aspect,1)}getViewBounds(e,t,i){zi.set(-1,-1,.5).applyMatrix4(this.projectionMatrixInverse),t.set(zi.x,zi.y).multiplyScalar(-e/zi.z),zi.set(1,1,.5).applyMatrix4(this.projectionMatrixInverse),i.set(zi.x,zi.y).multiplyScalar(-e/zi.z)}getViewSize(e,t){return this.getViewBounds(e,Rh,Ch),t.subVectors(Ch,Rh)}setViewOffset(e,t,i,r,s,o){this.aspect=e/t,this.view===null&&(this.view={enabled:!0,fullWidth:1,fullHeight:1,offsetX:0,offsetY:0,width:1,height:1}),this.view.enabled=!0,this.view.fullWidth=e,this.view.fullHeight=t,this.view.offsetX=i,this.view.offsetY=r,this.view.width=s,this.view.height=o,this.updateProjectionMatrix()}clearViewOffset(){this.view!==null&&(this.view.enabled=!1),this.updateProjectionMatrix()}updateProjectionMatrix(){let e=this.near,t=e*Math.tan(Wr*.5*this.fov)/this.zoom,i=2*t,r=this.aspect*i,s=-.5*r,o=this.view;if(this.view!==null&&this.view.enabled){let c=o.fullWidth,l=o.fullHeight;s+=o.offsetX*r/c,t-=o.offsetY*i/l,r*=o.width/c,i*=o.height/l}let a=this.filmOffset;a!==0&&(s+=e*a/this.getFilmWidth()),this.projectionMatrix.makePerspective(s,s+r,t,t-i,e,this.far,this.coordinateSystem,this.reversedDepth),this.projectionMatrixInverse.copy(this.projectionMatrix).invert()}toJSON(e){let t=super.toJSON(e);return t.object.fov=this.fov,t.object.zoom=this.zoom,t.object.near=this.near,t.object.far=this.far,t.object.focus=this.focus,t.object.aspect=this.aspect,this.view!==null&&(t.object.view=Object.assign({},this.view)),t.object.filmGauge=this.filmGauge,t.object.filmOffset=this.filmOffset,t}};var Mc=class extends $s{constructor(){super(new mn(90,1,.5,500)),this.isPointLightShadow=!0}},Zs=class extends is{constructor(e,t,i=0,r=2){super(e,t),this.isPointLight=!0,this.type="PointLight",this.distance=i,this.decay=r,this.shadow=new Mc}get power(){return this.intensity*4*Math.PI}set power(e){this.intensity=e/(4*Math.PI)}dispose(){super.dispose(),this.shadow.dispose()}copy(e,t){return super.copy(e,t),this.distance=e.distance,this.decay=e.decay,this.shadow=e.shadow.clone(),this}toJSON(e){let t=super.toJSON(e);return t.object.distance=this.distance,t.object.decay=this.decay,t.object.shadow=this.shadow.toJSON(),t}},rs=class extends Ys{constructor(e=-1,t=1,i=1,r=-1,s=.1,o=2e3){super(),this.isOrthographicCamera=!0,this.type="OrthographicCamera",this.zoom=1,this.view=null,this.left=e,this.right=t,this.top=i,this.bottom=r,this.near=s,this.far=o,this.updateProjectionMatrix()}copy(e,t){return super.copy(e,t),this.left=e.left,this.right=e.right,this.top=e.top,this.bottom=e.bottom,this.near=e.near,this.far=e.far,this.zoom=e.zoom,this.view=e.view===null?null:Object.assign({},e.view),this}setViewOffset(e,t,i,r,s,o){this.view===null&&(this.view={enabled:!0,fullWidth:1,fullHeight:1,offsetX:0,offsetY:0,width:1,height:1}),this.view.enabled=!0,this.view.fullWidth=e,this.view.fullHeight=t,this.view.offsetX=i,this.view.offsetY=r,this.view.width=s,this.view.height=o,this.updateProjectionMatrix()}clearViewOffset(){this.view!==null&&(this.view.enabled=!1),this.updateProjectionMatrix()}updateProjectionMatrix(){let e=(this.right-this.left)/(2*this.zoom),t=(this.top-this.bottom)/(2*this.zoom),i=(this.right+this.left)/2,r=(this.top+this.bottom)/2,s=i-e,o=i+e,a=r+t,c=r-t;if(this.view!==null&&this.view.enabled){let l=(this.right-this.left)/this.view.fullWidth/this.zoom,h=(this.top-this.bottom)/this.view.fullHeight/this.zoom;s+=l*this.view.offsetX,o=s+l*this.view.width,a-=h*this.view.offsetY,c=a-h*this.view.height}this.projectionMatrix.makeOrthographic(s,o,a,c,this.near,this.far,this.coordinateSystem,this.reversedDepth),this.projectionMatrixInverse.copy(this.projectionMatrix).invert()}toJSON(e){let t=super.toJSON(e);return t.object.zoom=this.zoom,t.object.left=this.left,t.object.right=this.right,t.object.top=this.top,t.object.bottom=this.bottom,t.object.near=this.near,t.object.far=this.far,this.view!==null&&(t.object.view=Object.assign({},this.view)),t}},Sc=class extends $s{constructor(){super(new rs(-5,5,5,-5,.5,500)),this.isDirectionalLightShadow=!0}},Ks=class extends is{constructor(e,t){super(e,t),this.isDirectionalLight=!0,this.type="DirectionalLight",this.position.copy(dn.DEFAULT_UP),this.updateMatrix(),this.target=new dn,this.shadow=new Sc}dispose(){super.dispose(),this.shadow.dispose()}copy(e){return super.copy(e),this.target=e.target.clone(),this.shadow=e.shadow.clone(),this}toJSON(e){let t=super.toJSON(e);return t.object.shadow=this.shadow.toJSON(),t.object.target=this.target.uuid,t}};var Hr=-90,Vr=1,Ra=class extends dn{constructor(e,t,i){super(),this.type="CubeCamera",this.renderTarget=i,this.coordinateSystem=null,this.activeMipmapLevel=0;let r=new mn(Hr,Vr,e,t);r.layers=this.layers,this.add(r);let s=new mn(Hr,Vr,e,t);s.layers=this.layers,this.add(s);let o=new mn(Hr,Vr,e,t);o.layers=this.layers,this.add(o);let a=new mn(Hr,Vr,e,t);a.layers=this.layers,this.add(a);let c=new mn(Hr,Vr,e,t);c.layers=this.layers,this.add(c);let l=new mn(Hr,Vr,e,t);l.layers=this.layers,this.add(l)}updateCoordinateSystem(){let e=this.coordinateSystem,t=this.children.concat(),[i,r,s,o,a,c]=t;for(let l of t)this.remove(l);if(e===ei)i.up.set(0,1,0),i.lookAt(1,0,0),r.up.set(0,1,0),r.lookAt(-1,0,0),s.up.set(0,0,-1),s.lookAt(0,1,0),o.up.set(0,0,1),o.lookAt(0,-1,0),a.up.set(0,1,0),a.lookAt(0,0,1),c.up.set(0,1,0),c.lookAt(0,0,-1);else if(e===$r)i.up.set(0,-1,0),i.lookAt(-1,0,0),r.up.set(0,-1,0),r.lookAt(1,0,0),s.up.set(0,0,1),s.lookAt(0,1,0),o.up.set(0,0,-1),o.lookAt(0,-1,0),a.up.set(0,-1,0),a.lookAt(0,0,1),c.up.set(0,-1,0),c.lookAt(0,0,-1);else throw new Error("THREE.CubeCamera.updateCoordinateSystem(): Invalid coordinate system: "+e);for(let l of t)this.add(l),l.updateMatrixWorld()}update(e,t){this.parent===null&&this.updateMatrixWorld();let{renderTarget:i,activeMipmapLevel:r}=this;this.coordinateSystem!==e.coordinateSystem&&(this.coordinateSystem=e.coordinateSystem,this.updateCoordinateSystem());let[s,o,a,c,l,h]=this.children,m=e.getRenderTarget(),u=e.getActiveCubeFace(),g=e.getActiveMipmapLevel(),_=e.xr.enabled;e.xr.enabled=!1;let x=i.texture.generateMipmaps;i.texture.generateMipmaps=!1;let f=!1;e.isWebGLRenderer===!0?f=e.state.buffers.depth.getReversed():f=e.reversedDepthBuffer,e.setRenderTarget(i,0,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,s),e.setRenderTarget(i,1,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,o),e.setRenderTarget(i,2,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,a),e.setRenderTarget(i,3,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,c),e.setRenderTarget(i,4,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,l),i.texture.generateMipmaps=x,e.setRenderTarget(i,5,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,h),e.setRenderTarget(m,u,g),e.xr.enabled=_,i.texture.needsPMREMUpdate=!0}},Ca=class extends mn{constructor(e=[]){super(),this.isArrayCamera=!0,this.isMultiViewCamera=!1,this.cameras=e}};var Qc="\\[\\]\\.:\\/",em=new RegExp("["+Qc+"]","g"),eu="[^"+Qc+"]",tm="[^"+Qc.replace("\\.","")+"]",nm=/((?:WC+[\/:])*)/.source.replace("WC",eu),im=/(WCOD+)?/.source.replace("WCOD",tm),rm=/(?:\.(WC+)(?:\[(.+)\])?)?/.source.replace("WC",eu),sm=/\.(WC+)(?:\[(.+)\])?/.source.replace("WC",eu),om=new RegExp("^"+nm+im+rm+sm+"$"),am=["material","materials","bones","map"],wc=class{constructor(e,t,i){let r=i||kt.parseTrackName(t);this._targetGroup=e,this._bindings=e.subscribe_(t,r)}getValue(e,t){this.bind();let i=this._targetGroup.nCachedObjects_,r=this._bindings[i];r!==void 0&&r.getValue(e,t)}setValue(e,t){let i=this._bindings;for(let r=this._targetGroup.nCachedObjects_,s=i.length;r!==s;++r)i[r].setValue(e,t)}bind(){let e=this._bindings;for(let t=this._targetGroup.nCachedObjects_,i=e.length;t!==i;++t)e[t].bind()}unbind(){let e=this._bindings;for(let t=this._targetGroup.nCachedObjects_,i=e.length;t!==i;++t)e[t].unbind()}},kt=class n{constructor(e,t,i){this.path=t,this.parsedPath=i||n.parseTrackName(t),this.node=n.findNode(e,this.parsedPath.nodeName),this.rootNode=e,this.getValue=this._getValue_unbound,this.setValue=this._setValue_unbound}static create(e,t,i){return e&&e.isAnimationObjectGroup?new n.Composite(e,t,i):new n(e,t,i)}static sanitizeNodeName(e){return e.replace(/\s/g,"_").replace(em,"")}static parseTrackName(e){let t=om.exec(e);if(t===null)throw new Error("THREE.PropertyBinding: Cannot parse trackName: "+e);let i={nodeName:t[2],objectName:t[3],objectIndex:t[4],propertyName:t[5],propertyIndex:t[6]},r=i.nodeName&&i.nodeName.lastIndexOf(".");if(r!==void 0&&r!==-1){let s=i.nodeName.substring(r+1);am.indexOf(s)!==-1&&(i.nodeName=i.nodeName.substring(0,r),i.objectName=s)}if(i.propertyName===null||i.propertyName.length===0)throw new Error("THREE.PropertyBinding: can not parse propertyName from trackName: "+e);return i}static findNode(e,t){if(t===void 0||t===""||t==="."||t===-1||t===e.name||t===e.uuid)return e;if(e.skeleton){let i=e.skeleton.getBoneByName(t);if(i!==void 0)return i}if(e.children){let i=function(s){for(let o=0;o<s.length;o++){let a=s[o];if(a.name===t||a.uuid===t)return a;let c=i(a.children);if(c)return c}return null},r=i(e.children);if(r)return r}return null}_getValue_unavailable(){}_setValue_unavailable(){}_getValue_direct(e,t){e[t]=this.targetObject[this.propertyName]}_getValue_array(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)e[t++]=i[r]}_getValue_arrayElement(e,t){e[t]=this.resolvedProperty[this.propertyIndex]}_getValue_toArray(e,t){this.resolvedProperty.toArray(e,t)}_setValue_direct(e,t){this.targetObject[this.propertyName]=e[t]}_setValue_direct_setNeedsUpdate(e,t){this.targetObject[this.propertyName]=e[t],this.targetObject.needsUpdate=!0}_setValue_direct_setMatrixWorldNeedsUpdate(e,t){this.targetObject[this.propertyName]=e[t],this.targetObject.matrixWorldNeedsUpdate=!0}_setValue_array(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)i[r]=e[t++]}_setValue_array_setNeedsUpdate(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)i[r]=e[t++];this.targetObject.needsUpdate=!0}_setValue_array_setMatrixWorldNeedsUpdate(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)i[r]=e[t++];this.targetObject.matrixWorldNeedsUpdate=!0}_setValue_arrayElement(e,t){this.resolvedProperty[this.propertyIndex]=e[t]}_setValue_arrayElement_setNeedsUpdate(e,t){this.resolvedProperty[this.propertyIndex]=e[t],this.targetObject.needsUpdate=!0}_setValue_arrayElement_setMatrixWorldNeedsUpdate(e,t){this.resolvedProperty[this.propertyIndex]=e[t],this.targetObject.matrixWorldNeedsUpdate=!0}_setValue_fromArray(e,t){this.resolvedProperty.fromArray(e,t)}_setValue_fromArray_setNeedsUpdate(e,t){this.resolvedProperty.fromArray(e,t),this.targetObject.needsUpdate=!0}_setValue_fromArray_setMatrixWorldNeedsUpdate(e,t){this.resolvedProperty.fromArray(e,t),this.targetObject.matrixWorldNeedsUpdate=!0}_getValue_unbound(e,t){this.bind(),this.getValue(e,t)}_setValue_unbound(e,t){this.bind(),this.setValue(e,t)}bind(){let e=this.node,t=this.parsedPath,i=t.objectName,r=t.propertyName,s=t.propertyIndex;if(e||(e=n.findNode(this.rootNode,t.nodeName),this.node=e),this.getValue=this._getValue_unavailable,this.setValue=this._setValue_unavailable,!e){Ge("PropertyBinding: No target node found for track: "+this.path+".");return}if(i){let l=t.objectIndex;switch(i){case"materials":if(!e.material){Xe("PropertyBinding: Can not bind to material as node does not have a material.",this);return}if(!e.material.materials){Xe("PropertyBinding: Can not bind to material.materials as node.material does not have a materials array.",this);return}e=e.material.materials;break;case"bones":if(!e.skeleton){Xe("PropertyBinding: Can not bind to bones as node does not have a skeleton.",this);return}e=e.skeleton.bones;for(let h=0;h<e.length;h++)if(e[h].name===l){l=h;break}break;case"map":if("map"in e){e=e.map;break}if(!e.material){Xe("PropertyBinding: Can not bind to material as node does not have a material.",this);return}if(!e.material.map){Xe("PropertyBinding: Can not bind to material.map as node.material does not have a map.",this);return}e=e.material.map;break;default:if(e[i]===void 0){Xe("PropertyBinding: Can not bind to objectName of node undefined.",this);return}e=e[i]}if(l!==void 0){if(e[l]===void 0){Xe("PropertyBinding: Trying to bind to objectIndex of objectName, but is undefined.",this,e);return}e=e[l]}}let o=e[r];if(o===void 0){let l=t.nodeName;Xe("PropertyBinding: Trying to update property for track: "+l+"."+r+" but it wasn't found.",e);return}let a=this.Versioning.None;this.targetObject=e,e.isMaterial===!0?a=this.Versioning.NeedsUpdate:e.isObject3D===!0&&(a=this.Versioning.MatrixWorldNeedsUpdate);let c=this.BindingType.Direct;if(s!==void 0){if(r==="morphTargetInfluences"){if(!e.geometry){Xe("PropertyBinding: Can not bind to morphTargetInfluences because node does not have a geometry.",this);return}if(!e.geometry.morphAttributes){Xe("PropertyBinding: Can not bind to morphTargetInfluences because node does not have a geometry.morphAttributes.",this);return}e.morphTargetDictionary[s]!==void 0&&(s=e.morphTargetDictionary[s])}c=this.BindingType.ArrayElement,this.resolvedProperty=o,this.propertyIndex=s}else o.fromArray!==void 0&&o.toArray!==void 0?(c=this.BindingType.HasFromToArray,this.resolvedProperty=o):Array.isArray(o)?(c=this.BindingType.EntireArray,this.resolvedProperty=o):this.propertyName=r;this.getValue=this.GetterByBindingType[c],this.setValue=this.SetterByBindingTypeAndVersioning[c][a]}unbind(){this.node=null,this.getValue=this._getValue_unbound,this.setValue=this._setValue_unbound}};kt.Composite=wc;kt.prototype.BindingType={Direct:0,EntireArray:1,ArrayElement:2,HasFromToArray:3};kt.prototype.Versioning={None:0,NeedsUpdate:1,MatrixWorldNeedsUpdate:2};kt.prototype.GetterByBindingType=[kt.prototype._getValue_direct,kt.prototype._getValue_array,kt.prototype._getValue_arrayElement,kt.prototype._getValue_toArray];kt.prototype.SetterByBindingTypeAndVersioning=[[kt.prototype._setValue_direct,kt.prototype._setValue_direct_setNeedsUpdate,kt.prototype._setValue_direct_setMatrixWorldNeedsUpdate],[kt.prototype._setValue_array,kt.prototype._setValue_array_setNeedsUpdate,kt.prototype._setValue_array_setMatrixWorldNeedsUpdate],[kt.prototype._setValue_arrayElement,kt.prototype._setValue_arrayElement_setNeedsUpdate,kt.prototype._setValue_arrayElement_setMatrixWorldNeedsUpdate],[kt.prototype._setValue_fromArray,kt.prototype._setValue_fromArray_setNeedsUpdate,kt.prototype._setValue_fromArray_setMatrixWorldNeedsUpdate]];var ob=new Float32Array(1);var Ph=new It,Js=class{constructor(e,t,i=0,r=1/0){this.ray=new Ri(e,t),this.near=i,this.far=r,this.camera=null,this.layers=new Jr,this.params={Mesh:{},Line:{threshold:1},LOD:{},Points:{threshold:1},Sprite:{}}}set(e,t){this.ray.set(e,t)}setFromCamera(e,t){t.isPerspectiveCamera?(this.ray.origin.setFromMatrixPosition(t.matrixWorld),this.ray.direction.set(e.x,e.y,.5).unproject(t).sub(this.ray.origin).normalize(),this.camera=t):t.isOrthographicCamera?(this.ray.origin.set(e.x,e.y,t.projectionMatrix.elements[14]).unproject(t),this.ray.direction.set(0,0,-1).transformDirection(t.matrixWorld),this.camera=t):Xe("Raycaster: Unsupported camera type: "+t.type)}setFromXRController(e){return Ph.identity().extractRotation(e.matrixWorld),this.ray.origin.setFromMatrixPosition(e.matrixWorld),this.ray.direction.set(0,0,-1).applyMatrix4(Ph),this}intersectObject(e,t=!0,i=[]){return Ec(e,this,i,t),i.sort(Ih),i}intersectObjects(e,t=!0,i=[]){for(let r=0,s=e.length;r<s;r++)Ec(e[r],this,i,t);return i.sort(Ih),i}};function Ih(n,e){return n.distance-e.distance}function Ec(n,e,t,i){let r=!0;if(n.layers.test(e.layers)&&n.raycast(e,t)===!1&&(r=!1),r===!0&&i===!0){let s=n.children;for(let o=0,a=s.length;o<a;o++)Ec(s[o],e,t,!0)}}var ss=class{constructor(e=1,t=0,i=0){this.radius=e,this.phi=t,this.theta=i}set(e,t,i){return this.radius=e,this.phi=t,this.theta=i,this}copy(e){return this.radius=e.radius,this.phi=e.phi,this.theta=e.theta,this}makeSafe(){return this.phi=it(this.phi,1e-6,Math.PI-1e-6),this}setFromVector3(e){return this.setFromCartesianCoords(e.x,e.y,e.z)}setFromCartesianCoords(e,t,i){return this.radius=Math.sqrt(e*e+t*t+i*i),this.radius===0?(this.theta=0,this.phi=0):(this.theta=Math.atan2(e,i),this.phi=Math.acos(it(t/this.radius,-1,1))),this}clone(){return new this.constructor().copy(this)}};var Tc=class n{static{n.prototype.isMatrix2=!0}constructor(e,t,i,r){this.elements=[1,0,0,1],e!==void 0&&this.set(e,t,i,r)}identity(){return this.set(1,0,0,1),this}fromArray(e,t=0){for(let i=0;i<4;i++)this.elements[i]=e[i+t];return this}set(e,t,i,r){let s=this.elements;return s[0]=e,s[2]=t,s[1]=i,s[3]=r,this}};var js=class extends es{constructor(e=10,t=10,i=4473924,r=8947848){i=new ze(i),r=new ze(r);let s=t/2,o=e/t,a=e/2,c=[],l=[];for(let u=0,g=0,_=-a;u<=t;u++,_+=o){c.push(-a,0,_,a,0,_),c.push(_,0,-a,_,0,a);let x=u===s?i:r;x.toArray(l,g),g+=3,x.toArray(l,g),g+=3,x.toArray(l,g),g+=3,x.toArray(l,g),g+=3}let h=new Xt;h.setAttribute("position",new ct(c,3)),h.setAttribute("color",new ct(l,3));let m=new hr({vertexColors:!0,toneMapped:!1});super(h,m),this.type="GridHelper"}dispose(){super.dispose(),this.geometry.dispose(),this.material.dispose()}};var Qs=class extends ti{constructor(e,t=null){super(),this.object=e,this.domElement=t,this.enabled=!0,this.state=-1,this.keys={},this.mouseButtons={LEFT:null,MIDDLE:null,RIGHT:null},this.touches={ONE:null,TWO:null}}connect(e){this.domElement!==null&&this.disconnect(),this.domElement=e}disconnect(){}dispose(){}update(){}};function tu(n,e,t,i){let r=lm(i);switch(t){case Xc:return n*e;case $c:return n*e/r.components*r.byteLength;case Fa:return n*e/r.components*r.byteLength;case Qi:return n*e*2/r.components*r.byteLength;case Oa:return n*e*2/r.components*r.byteLength;case qc:return n*e*3/r.components*r.byteLength;case $n:return n*e*4/r.components*r.byteLength;case Ba:return n*e*4/r.components*r.byteLength;case io:case ro:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*8;case so:case oo:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*16;case Ha:case Ga:return Math.max(n,16)*Math.max(e,8)/4;case za:case Va:return Math.max(n,8)*Math.max(e,8)/2;case Wa:case Xa:case $a:case Ya:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*8;case qa:case ao:case Za:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*16;case Ka:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*16;case Ja:return Math.floor((n+4)/5)*Math.floor((e+3)/4)*16;case ja:return Math.floor((n+4)/5)*Math.floor((e+4)/5)*16;case Qa:return Math.floor((n+5)/6)*Math.floor((e+4)/5)*16;case el:return Math.floor((n+5)/6)*Math.floor((e+5)/6)*16;case tl:return Math.floor((n+7)/8)*Math.floor((e+4)/5)*16;case nl:return Math.floor((n+7)/8)*Math.floor((e+5)/6)*16;case il:return Math.floor((n+7)/8)*Math.floor((e+7)/8)*16;case rl:return Math.floor((n+9)/10)*Math.floor((e+4)/5)*16;case sl:return Math.floor((n+9)/10)*Math.floor((e+5)/6)*16;case ol:return Math.floor((n+9)/10)*Math.floor((e+7)/8)*16;case al:return Math.floor((n+9)/10)*Math.floor((e+9)/10)*16;case ll:return Math.floor((n+11)/12)*Math.floor((e+9)/10)*16;case cl:return Math.floor((n+11)/12)*Math.floor((e+11)/12)*16;case ul:case hl:case dl:return Math.ceil(n/4)*Math.ceil(e/4)*16;case fl:case pl:return Math.ceil(n/4)*Math.ceil(e/4)*8;case lo:case ml:return Math.ceil(n/4)*Math.ceil(e/4)*16}throw new Error(`Unable to determine texture byte length for ${t} format.`)}function lm(n){switch(n){case Ln:case Hc:return{byteLength:1,components:1};case cs:case Vc:case si:return{byteLength:2,components:1};case Na:case Ua:return{byteLength:2,components:4};case ii:case Da:case ri:return{byteLength:4,components:1};case Gc:case Wc:return{byteLength:4,components:3}}throw new Error(`THREE.TextureUtils: Unknown texture type ${n}.`)}typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("register",{detail:{revision:"186"}}));typeof window<"u"&&(window.__THREE__?Ge("WARNING: Multiple instances of Three.js being imported."):window.__THREE__="186");function zd(){let n=null,e=!1,t=null,i=null;function r(s,o){i=n.requestAnimationFrame(r),t(s,o)}return{start:function(){e!==!0&&t!==null&&n!==null&&(i=n.requestAnimationFrame(r),e=!0)},stop:function(){n!==null&&n.cancelAnimationFrame(i),e=!1},setAnimationLoop:function(s){t=s},setContext:function(s){n=s}}}function um(n){let e=new WeakMap;function t(a,c){let l=a.array,h=a.usage,m=l.byteLength,u=n.createBuffer();n.bindBuffer(c,u),n.bufferData(c,l,h),a.onUploadCallback();let g;if(l instanceof Float32Array)g=n.FLOAT;else if(typeof Float16Array<"u"&&l instanceof Float16Array)g=n.HALF_FLOAT;else if(l instanceof Uint16Array)a.isFloat16BufferAttribute?g=n.HALF_FLOAT:g=n.UNSIGNED_SHORT;else if(l instanceof Int16Array)g=n.SHORT;else if(l instanceof Uint32Array)g=n.UNSIGNED_INT;else if(l instanceof Int32Array)g=n.INT;else if(l instanceof Int8Array)g=n.BYTE;else if(l instanceof Uint8Array)g=n.UNSIGNED_BYTE;else if(l instanceof Uint8ClampedArray)g=n.UNSIGNED_BYTE;else throw new Error("THREE.WebGLAttributes: Unsupported buffer data format: "+l);return{buffer:u,type:g,bytesPerElement:l.BYTES_PER_ELEMENT,version:a.version,size:m}}function i(a,c,l){let h=c.array,m=c.updateRanges;if(n.bindBuffer(l,a),m.length===0)n.bufferSubData(l,0,h);else{m.sort((g,_)=>g.start-_.start);let u=0;for(let g=1;g<m.length;g++){let _=m[u],x=m[g];x.start<=_.start+_.count+1?_.count=Math.max(_.count,x.start+x.count-_.start):(++u,m[u]=x)}m.length=u+1;for(let g=0,_=m.length;g<_;g++){let x=m[g];n.bufferSubData(l,x.start*h.BYTES_PER_ELEMENT,h,x.start,x.count)}c.clearUpdateRanges()}c.onUploadCallback()}function r(a){return a.isInterleavedBufferAttribute&&(a=a.data),e.get(a)}function s(a){a.isInterleavedBufferAttribute&&(a=a.data);let c=e.get(a);c&&(n.deleteBuffer(c.buffer),e.delete(a))}function o(a,c){if(a.isInterleavedBufferAttribute&&(a=a.data),a.isGLBufferAttribute){let h=e.get(a);(!h||h.version<a.version)&&e.set(a,{buffer:a.buffer,type:a.type,bytesPerElement:a.elementSize,version:a.version});return}let l=e.get(a);if(l===void 0)e.set(a,t(a,c));else if(l.version<a.version){if(l.size!==a.array.byteLength)throw new Error("THREE.WebGLAttributes: The size of the buffer attribute's array buffer does not match the original size. Resizing buffer attributes is not supported.");i(l.buffer,a,c),l.version=a.version}}return{get:r,remove:s,update:o}}var hm=`#ifdef USE_ALPHAHASH
	if ( diffuseColor.a < getAlphaHashThreshold( vPosition ) ) discard;
#endif`,dm=`#ifdef USE_ALPHAHASH
	const float ALPHA_HASH_SCALE = 0.05;
	float hash2D( vec2 value ) {
		return fract( 1.0e4 * sin( 17.0 * value.x + 0.1 * value.y ) * ( 0.1 + abs( sin( 13.0 * value.y + value.x ) ) ) );
	}
	float hash3D( vec3 value ) {
		return hash2D( vec2( hash2D( value.xy ), value.z ) );
	}
	float getAlphaHashThreshold( vec3 position ) {
		float maxDeriv = max(
			length( dFdx( position.xyz ) ),
			length( dFdy( position.xyz ) )
		);
		float pixScale = 1.0 / ( ALPHA_HASH_SCALE * maxDeriv );
		vec2 pixScales = vec2(
			exp2( floor( log2( pixScale ) ) ),
			exp2( ceil( log2( pixScale ) ) )
		);
		vec2 alpha = vec2(
			hash3D( floor( pixScales.x * position.xyz ) ),
			hash3D( floor( pixScales.y * position.xyz ) )
		);
		float lerpFactor = fract( log2( pixScale ) );
		float x = ( 1.0 - lerpFactor ) * alpha.x + lerpFactor * alpha.y;
		float a = min( lerpFactor, 1.0 - lerpFactor );
		vec3 cases = vec3(
			x * x / ( 2.0 * a * ( 1.0 - a ) ),
			( x - 0.5 * a ) / ( 1.0 - a ),
			1.0 - ( ( 1.0 - x ) * ( 1.0 - x ) / ( 2.0 * a * ( 1.0 - a ) ) )
		);
		float threshold = ( x < ( 1.0 - a ) )
			? ( ( x < a ) ? cases.x : cases.y )
			: cases.z;
		return clamp( threshold , 1.0e-6, 1.0 );
	}
#endif`,fm=`#ifdef USE_ALPHAMAP
	diffuseColor.a *= texture2D( alphaMap, vAlphaMapUv ).g;
#endif`,pm=`#ifdef USE_ALPHAMAP
	uniform sampler2D alphaMap;
#endif`,mm=`#ifdef USE_ALPHATEST
	#ifdef ALPHA_TO_COVERAGE
	diffuseColor.a = smoothstep( alphaTest, alphaTest + fwidth( diffuseColor.a ), diffuseColor.a );
	if ( diffuseColor.a == 0.0 ) discard;
	#else
	if ( diffuseColor.a < alphaTest ) discard;
	#endif
#endif`,gm=`#ifdef USE_ALPHATEST
	uniform float alphaTest;
#endif`,xm=`#ifdef USE_AOMAP
	float ambientOcclusion = ( texture2D( aoMap, vAoMapUv ).r - 1.0 ) * aoMapIntensity + 1.0;
	reflectedLight.indirectDiffuse *= ambientOcclusion;
	#if defined( USE_CLEARCOAT ) 
		clearcoatSpecularIndirect *= ambientOcclusion;
	#endif
	#if defined( USE_SHEEN ) 
		sheenSpecularIndirect *= ambientOcclusion;
	#endif
	#if defined( USE_ENVMAP ) && defined( STANDARD )
		float dotNV = saturate( dot( geometryNormal, geometryViewDir ) );
		reflectedLight.indirectSpecular *= computeSpecularOcclusion( dotNV, ambientOcclusion, material.roughness );
	#endif
#endif`,vm=`#ifdef USE_AOMAP
	uniform sampler2D aoMap;
	uniform float aoMapIntensity;
#endif`,_m=`#ifdef USE_BATCHING
	#if ! defined( GL_ANGLE_multi_draw )
	#define gl_DrawID _gl_DrawID
	uniform int _gl_DrawID;
	#endif
	uniform highp sampler2D batchingTexture;
	uniform highp usampler2D batchingIdTexture;
	mat4 getBatchingMatrix( const in float i ) {
		int size = textureSize( batchingTexture, 0 ).x;
		int j = int( i ) * 4;
		int x = j % size;
		int y = j / size;
		vec4 v1 = texelFetch( batchingTexture, ivec2( x, y ), 0 );
		vec4 v2 = texelFetch( batchingTexture, ivec2( x + 1, y ), 0 );
		vec4 v3 = texelFetch( batchingTexture, ivec2( x + 2, y ), 0 );
		vec4 v4 = texelFetch( batchingTexture, ivec2( x + 3, y ), 0 );
		return mat4( v1, v2, v3, v4 );
	}
	float getIndirectIndex( const in int i ) {
		int size = textureSize( batchingIdTexture, 0 ).x;
		int x = i % size;
		int y = i / size;
		return float( texelFetch( batchingIdTexture, ivec2( x, y ), 0 ).r );
	}
#endif
#ifdef USE_BATCHING_COLOR
	uniform sampler2D batchingColorTexture;
	vec4 getBatchingColor( const in float i ) {
		int size = textureSize( batchingColorTexture, 0 ).x;
		int j = int( i );
		int x = j % size;
		int y = j / size;
		return texelFetch( batchingColorTexture, ivec2( x, y ), 0 );
	}
#endif`,ym=`#ifdef USE_BATCHING
	mat4 batchingMatrix = getBatchingMatrix( getIndirectIndex( gl_DrawID ) );
#endif`,bm=`vec3 transformed = vec3( position );
#ifdef USE_ALPHAHASH
	vPosition = vec3( position );
#endif`,Mm=`vec3 objectNormal = vec3( normal );
#ifdef USE_TANGENT
	vec3 objectTangent = vec3( tangent.xyz );
#endif`,Sm=`float G_BlinnPhong_Implicit( ) {
	return 0.25;
}
float D_BlinnPhong( const in float shininess, const in float dotNH ) {
	return RECIPROCAL_PI * ( shininess * 0.5 + 1.0 ) * pow( dotNH, shininess );
}
vec3 BRDF_BlinnPhong( const in vec3 lightDir, const in vec3 viewDir, const in vec3 normal, const in vec3 specularColor, const in float shininess ) {
	vec3 halfDir = normalize( lightDir + viewDir );
	float dotNH = saturate( dot( normal, halfDir ) );
	float dotVH = saturate( dot( viewDir, halfDir ) );
	vec3 F = F_Schlick( specularColor, 1.0, dotVH );
	float G = G_BlinnPhong_Implicit( );
	float D = D_BlinnPhong( shininess, dotNH );
	return F * ( G * D );
} // validated`,wm=`#ifdef USE_IRIDESCENCE
	const mat3 XYZ_TO_REC709 = mat3(
		 3.2404542, -0.9692660,  0.0556434,
		-1.5371385,  1.8760108, -0.2040259,
		-0.4985314,  0.0415560,  1.0572252
	);
	vec3 Fresnel0ToIor( vec3 fresnel0 ) {
		vec3 sqrtF0 = sqrt( fresnel0 );
		return ( vec3( 1.0 ) + sqrtF0 ) / ( vec3( 1.0 ) - sqrtF0 );
	}
	vec3 IorToFresnel0( vec3 transmittedIor, float incidentIor ) {
		return pow2( ( transmittedIor - vec3( incidentIor ) ) / ( transmittedIor + vec3( incidentIor ) ) );
	}
	float IorToFresnel0( float transmittedIor, float incidentIor ) {
		return pow2( ( transmittedIor - incidentIor ) / ( transmittedIor + incidentIor ));
	}
	vec3 evalSensitivity( float OPD, vec3 shift ) {
		float phase = 2.0 * PI * OPD * 1.0e-9;
		vec3 val = vec3( 5.4856e-13, 4.4201e-13, 5.2481e-13 );
		vec3 pos = vec3( 1.6810e+06, 1.7953e+06, 2.2084e+06 );
		vec3 var = vec3( 4.3278e+09, 9.3046e+09, 6.6121e+09 );
		vec3 xyz = val * sqrt( 2.0 * PI * var ) * cos( pos * phase + shift ) * exp( - pow2( phase ) * var );
		xyz.x += 9.7470e-14 * sqrt( 2.0 * PI * 4.5282e+09 ) * cos( 2.2399e+06 * phase + shift[ 0 ] ) * exp( - 4.5282e+09 * pow2( phase ) );
		xyz /= 1.0685e-7;
		vec3 rgb = XYZ_TO_REC709 * xyz;
		return rgb;
	}
	vec3 evalIridescence( float outsideIOR, float eta2, float cosTheta1, float thinFilmThickness, vec3 baseF0 ) {
		vec3 I;
		float iridescenceIOR = mix( outsideIOR, eta2, smoothstep( 0.0, 0.03, thinFilmThickness ) );
		float sinTheta2Sq = pow2( outsideIOR / iridescenceIOR ) * ( 1.0 - pow2( cosTheta1 ) );
		float cosTheta2Sq = 1.0 - sinTheta2Sq;
		if ( cosTheta2Sq < 0.0 ) {
			return vec3( 1.0 );
		}
		float cosTheta2 = sqrt( cosTheta2Sq );
		float R0 = IorToFresnel0( iridescenceIOR, outsideIOR );
		float R12 = F_Schlick( R0, 1.0, cosTheta1 );
		float T121 = 1.0 - R12;
		float phi12 = 0.0;
		if ( iridescenceIOR < outsideIOR ) phi12 = PI;
		float phi21 = PI - phi12;
		vec3 baseIOR = Fresnel0ToIor( clamp( baseF0, 0.0, 0.9999 ) );		vec3 R1 = IorToFresnel0( baseIOR, iridescenceIOR );
		vec3 R23 = F_Schlick( R1, 1.0, cosTheta2 );
		vec3 phi23 = vec3( 0.0 );
		if ( baseIOR[ 0 ] < iridescenceIOR ) phi23[ 0 ] = PI;
		if ( baseIOR[ 1 ] < iridescenceIOR ) phi23[ 1 ] = PI;
		if ( baseIOR[ 2 ] < iridescenceIOR ) phi23[ 2 ] = PI;
		float OPD = 2.0 * iridescenceIOR * thinFilmThickness * cosTheta2;
		vec3 phi = vec3( phi21 ) + phi23;
		vec3 R123 = clamp( R12 * R23, 1e-5, 0.9999 );
		vec3 r123 = sqrt( R123 );
		vec3 Rs = pow2( T121 ) * R23 / ( vec3( 1.0 ) - R123 );
		vec3 C0 = R12 + Rs;
		I = C0;
		vec3 Cm = Rs - T121;
		for ( int m = 1; m <= 2; ++ m ) {
			Cm *= r123;
			vec3 Sm = 2.0 * evalSensitivity( float( m ) * OPD, float( m ) * phi );
			I += Cm * Sm;
		}
		return max( I, vec3( 0.0 ) );
	}
#endif`,Em=`#ifdef USE_BUMPMAP
	uniform sampler2D bumpMap;
	uniform float bumpScale;
	vec2 dHdxy_fwd() {
		vec2 dSTdx = dFdx( vBumpMapUv );
		vec2 dSTdy = dFdy( vBumpMapUv );
		float Hll = bumpScale * texture2D( bumpMap, vBumpMapUv ).x;
		float dBx = bumpScale * texture2D( bumpMap, vBumpMapUv + dSTdx ).x - Hll;
		float dBy = bumpScale * texture2D( bumpMap, vBumpMapUv + dSTdy ).x - Hll;
		return vec2( dBx, dBy );
	}
	vec3 perturbNormalArb( vec3 surf_pos, vec3 surf_norm, vec2 dHdxy, float faceDirection ) {
		vec3 vSigmaX = normalize( dFdx( surf_pos.xyz ) );
		vec3 vSigmaY = normalize( dFdy( surf_pos.xyz ) );
		vec3 vN = surf_norm;
		vec3 R1 = cross( vSigmaY, vN );
		vec3 R2 = cross( vN, vSigmaX );
		float fDet = dot( vSigmaX, R1 ) * faceDirection;
		vec3 vGrad = sign( fDet ) * ( dHdxy.x * R1 + dHdxy.y * R2 );
		return normalize( abs( fDet ) * surf_norm - vGrad );
	}
#endif`,Tm=`#if NUM_CLIPPING_PLANES > 0
	vec4 plane;
	#ifdef ALPHA_TO_COVERAGE
		float distanceToPlane, distanceGradient;
		float clipOpacity = 1.0;
		#pragma unroll_loop_start
		for ( int i = 0; i < UNION_CLIPPING_PLANES; i ++ ) {
			plane = clippingPlanes[ i ];
			distanceToPlane = - dot( vClipPosition, plane.xyz ) + plane.w;
			distanceGradient = fwidth( distanceToPlane ) / 2.0;
			clipOpacity *= smoothstep( - distanceGradient, distanceGradient, distanceToPlane );
			if ( clipOpacity == 0.0 ) discard;
		}
		#pragma unroll_loop_end
		#if UNION_CLIPPING_PLANES < NUM_CLIPPING_PLANES
			float unionClipOpacity = 1.0;
			#pragma unroll_loop_start
			for ( int i = UNION_CLIPPING_PLANES; i < NUM_CLIPPING_PLANES; i ++ ) {
				plane = clippingPlanes[ i ];
				distanceToPlane = - dot( vClipPosition, plane.xyz ) + plane.w;
				distanceGradient = fwidth( distanceToPlane ) / 2.0;
				unionClipOpacity *= 1.0 - smoothstep( - distanceGradient, distanceGradient, distanceToPlane );
			}
			#pragma unroll_loop_end
			clipOpacity *= 1.0 - unionClipOpacity;
		#endif
		diffuseColor.a *= clipOpacity;
		if ( diffuseColor.a == 0.0 ) discard;
	#else
		#pragma unroll_loop_start
		for ( int i = 0; i < UNION_CLIPPING_PLANES; i ++ ) {
			plane = clippingPlanes[ i ];
			if ( dot( vClipPosition, plane.xyz ) > plane.w ) discard;
		}
		#pragma unroll_loop_end
		#if UNION_CLIPPING_PLANES < NUM_CLIPPING_PLANES
			bool clipped = true;
			#pragma unroll_loop_start
			for ( int i = UNION_CLIPPING_PLANES; i < NUM_CLIPPING_PLANES; i ++ ) {
				plane = clippingPlanes[ i ];
				clipped = ( dot( vClipPosition, plane.xyz ) > plane.w ) && clipped;
			}
			#pragma unroll_loop_end
			if ( clipped ) discard;
		#endif
	#endif
#endif`,Am=`#if NUM_CLIPPING_PLANES > 0
	varying vec3 vClipPosition;
	uniform vec4 clippingPlanes[ NUM_CLIPPING_PLANES ];
#endif`,Rm=`#if NUM_CLIPPING_PLANES > 0
	varying vec3 vClipPosition;
#endif`,Cm=`#if NUM_CLIPPING_PLANES > 0
	vClipPosition = - mvPosition.xyz;
#endif`,Pm=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA )
	diffuseColor *= vColor;
#endif`,Im=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA )
	varying vec4 vColor;
#endif`,Lm=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA ) || defined( USE_INSTANCING_COLOR ) || defined( USE_BATCHING_COLOR )
	varying vec4 vColor;
#endif`,km=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA ) || defined( USE_INSTANCING_COLOR ) || defined( USE_BATCHING_COLOR )
	vColor = vec4( 1.0 );
#endif
#ifdef USE_COLOR_ALPHA
	vColor *= color;
#elif defined( USE_COLOR )
	vColor.rgb *= color;
#endif
#ifdef USE_INSTANCING_COLOR
	vColor.rgb *= instanceColor.rgb;
#endif
#ifdef USE_BATCHING_COLOR
	vColor *= getBatchingColor( getIndirectIndex( gl_DrawID ) );
#endif`,Dm=`#define PI 3.141592653589793
#define PI2 6.283185307179586
#define PI_HALF 1.5707963267948966
#define RECIPROCAL_PI 0.3183098861837907
#define RECIPROCAL_PI2 0.15915494309189535
#define EPSILON 1e-6
#ifndef saturate
#define saturate( a ) clamp( a, 0.0, 1.0 )
#endif
#define whiteComplement( a ) ( 1.0 - saturate( a ) )
float pow2( const in float x ) { return x*x; }
vec3 pow2( const in vec3 x ) { return x*x; }
float pow3( const in float x ) { return x*x*x; }
float pow4( const in float x ) { float x2 = x*x; return x2*x2; }
float max3( const in vec3 v ) { return max( max( v.x, v.y ), v.z ); }
float average( const in vec3 v ) { return dot( v, vec3( 0.3333333 ) ); }
highp float rand( const in vec2 uv ) {
	const highp float a = 12.9898, b = 78.233, c = 43758.5453;
	highp float dt = dot( uv.xy, vec2( a,b ) ), sn = mod( dt, PI );
	return fract( sin( sn ) * c );
}
#ifdef HIGH_PRECISION
	float precisionSafeLength( vec3 v ) { return length( v ); }
#else
	float precisionSafeLength( vec3 v ) {
		float maxComponent = max3( abs( v ) );
		return length( v / maxComponent ) * maxComponent;
	}
#endif
struct IncidentLight {
	vec3 color;
	vec3 direction;
	bool visible;
};
struct ReflectedLight {
	vec3 directDiffuse;
	vec3 directSpecular;
	vec3 indirectDiffuse;
	vec3 indirectSpecular;
};
#ifdef USE_ALPHAHASH
	varying vec3 vPosition;
#endif
vec3 transformDirection( in vec3 dir, in mat4 matrix ) {
	return normalize( ( matrix * vec4( dir, 0.0 ) ).xyz );
}
#define inverseTransformDirection transformDirectionByInverseViewMatrix
vec3 transformNormalByInverseViewMatrix( in vec3 normal, in mat4 viewMatrix ) {
	return normalize( ( vec4( normal, 0.0 ) * viewMatrix ).xyz );
}
vec3 transformDirectionByInverseViewMatrix( in vec3 dir, in mat4 viewMatrix ) {
	return normalize( ( vec4( dir, 0.0 ) * viewMatrix ).xyz );
}
bool isPerspectiveMatrix( mat4 m ) {
	return m[ 2 ][ 3 ] == - 1.0;
}
vec2 equirectUv( in vec3 dir ) {
	float u = atan( dir.z, dir.x ) * RECIPROCAL_PI2 + 0.5;
	float v = asin( clamp( dir.y, - 1.0, 1.0 ) ) * RECIPROCAL_PI + 0.5;
	return vec2( u, v );
}
vec3 BRDF_Lambert( const in vec3 diffuseColor ) {
	return RECIPROCAL_PI * diffuseColor;
}
vec3 F_Schlick( const in vec3 f0, const in float f90, const in float dotVH ) {
	float fresnel = exp2( ( - 5.55473 * dotVH - 6.98316 ) * dotVH );
	return f0 * ( 1.0 - fresnel ) + ( f90 * fresnel );
}
float F_Schlick( const in float f0, const in float f90, const in float dotVH ) {
	float fresnel = exp2( ( - 5.55473 * dotVH - 6.98316 ) * dotVH );
	return f0 * ( 1.0 - fresnel ) + ( f90 * fresnel );
} // validated`,Nm=`#ifdef ENVMAP_TYPE_CUBE_UV
	#define cubeUV_minMipLevel 4.0
	#define cubeUV_minTileSize 16.0
	float getFace( vec3 direction ) {
		vec3 absDirection = abs( direction );
		float face = - 1.0;
		if ( absDirection.x > absDirection.z ) {
			if ( absDirection.x > absDirection.y )
				face = direction.x > 0.0 ? 0.0 : 3.0;
			else
				face = direction.y > 0.0 ? 1.0 : 4.0;
		} else {
			if ( absDirection.z > absDirection.y )
				face = direction.z > 0.0 ? 2.0 : 5.0;
			else
				face = direction.y > 0.0 ? 1.0 : 4.0;
		}
		return face;
	}
	vec2 getUV( vec3 direction, float face ) {
		vec2 uv;
		if ( face == 0.0 ) {
			uv = vec2( direction.z, direction.y ) / abs( direction.x );
		} else if ( face == 1.0 ) {
			uv = vec2( - direction.x, - direction.z ) / abs( direction.y );
		} else if ( face == 2.0 ) {
			uv = vec2( - direction.x, direction.y ) / abs( direction.z );
		} else if ( face == 3.0 ) {
			uv = vec2( - direction.z, direction.y ) / abs( direction.x );
		} else if ( face == 4.0 ) {
			uv = vec2( - direction.x, direction.z ) / abs( direction.y );
		} else {
			uv = vec2( direction.x, direction.y ) / abs( direction.z );
		}
		return 0.5 * ( uv + 1.0 );
	}
	vec3 bilinearCubeUV( sampler2D envMap, vec3 direction, float mipInt ) {
		float face = getFace( direction );
		float filterInt = max( cubeUV_minMipLevel - mipInt, 0.0 );
		mipInt = max( mipInt, cubeUV_minMipLevel );
		float faceSize = exp2( mipInt );
		highp vec2 uv = getUV( direction, face ) * ( faceSize - 2.0 ) + 1.0;
		if ( face > 2.0 ) {
			uv.y += faceSize;
			face -= 3.0;
		}
		uv.x += face * faceSize;
		uv.x += filterInt * 3.0 * cubeUV_minTileSize;
		uv.y += 4.0 * ( exp2( CUBEUV_MAX_MIP ) - faceSize );
		uv.x *= CUBEUV_TEXEL_WIDTH;
		uv.y *= CUBEUV_TEXEL_HEIGHT;
		#ifdef texture2DGradEXT
			return texture2DGradEXT( envMap, uv, vec2( 0.0 ), vec2( 0.0 ) ).rgb;
		#else
			return texture2D( envMap, uv ).rgb;
		#endif
	}
	#define cubeUV_r0 1.0
	#define cubeUV_m0 - 2.0
	#define cubeUV_r1 0.8
	#define cubeUV_m1 - 1.0
	#define cubeUV_r4 0.4
	#define cubeUV_m4 2.0
	#define cubeUV_r5 0.305
	#define cubeUV_m5 3.0
	#define cubeUV_r6 0.21
	#define cubeUV_m6 4.0
	float roughnessToMip( float roughness ) {
		float mip = 0.0;
		if ( roughness >= cubeUV_r1 ) {
			mip = ( cubeUV_r0 - roughness ) * ( cubeUV_m1 - cubeUV_m0 ) / ( cubeUV_r0 - cubeUV_r1 ) + cubeUV_m0;
		} else if ( roughness >= cubeUV_r4 ) {
			mip = ( cubeUV_r1 - roughness ) * ( cubeUV_m4 - cubeUV_m1 ) / ( cubeUV_r1 - cubeUV_r4 ) + cubeUV_m1;
		} else if ( roughness >= cubeUV_r5 ) {
			mip = ( cubeUV_r4 - roughness ) * ( cubeUV_m5 - cubeUV_m4 ) / ( cubeUV_r4 - cubeUV_r5 ) + cubeUV_m4;
		} else if ( roughness >= cubeUV_r6 ) {
			mip = ( cubeUV_r5 - roughness ) * ( cubeUV_m6 - cubeUV_m5 ) / ( cubeUV_r5 - cubeUV_r6 ) + cubeUV_m5;
		} else {
			mip = - 2.0 * log2( 1.16 * roughness );		}
		return mip;
	}
	vec4 textureCubeUV( sampler2D envMap, vec3 sampleDir, float roughness ) {
		float mip = clamp( roughnessToMip( roughness ), cubeUV_m0, CUBEUV_MAX_MIP );
		float mipF = fract( mip );
		float mipInt = floor( mip );
		vec3 color0 = bilinearCubeUV( envMap, sampleDir, mipInt );
		if ( mipF == 0.0 ) {
			return vec4( color0, 1.0 );
		} else {
			vec3 color1 = bilinearCubeUV( envMap, sampleDir, mipInt + 1.0 );
			return vec4( mix( color0, color1, mipF ), 1.0 );
		}
	}
#endif`,Um=`vec3 transformedNormal = objectNormal;
#ifdef USE_TANGENT
	vec3 transformedTangent = objectTangent;
#endif
#ifdef USE_BATCHING
	mat3 bm = mat3( batchingMatrix );
	transformedNormal /= vec3( dot( bm[ 0 ], bm[ 0 ] ), dot( bm[ 1 ], bm[ 1 ] ), dot( bm[ 2 ], bm[ 2 ] ) );
	transformedNormal = bm * transformedNormal;
	#ifdef USE_TANGENT
		transformedTangent = bm * transformedTangent;
	#endif
#endif
#ifdef USE_INSTANCING
	mat3 im = mat3( instanceMatrix );
	transformedNormal /= vec3( dot( im[ 0 ], im[ 0 ] ), dot( im[ 1 ], im[ 1 ] ), dot( im[ 2 ], im[ 2 ] ) );
	transformedNormal = im * transformedNormal;
	#ifdef USE_TANGENT
		transformedTangent = im * transformedTangent;
	#endif
#endif
transformedNormal = normalMatrix * transformedNormal;
#ifdef FLIP_SIDED
	transformedNormal = - transformedNormal;
#endif
#ifdef USE_TANGENT
	transformedTangent = ( modelViewMatrix * vec4( transformedTangent, 0.0 ) ).xyz;
#endif`,Fm=`#ifdef USE_DISPLACEMENTMAP
	uniform sampler2D displacementMap;
	uniform float displacementScale;
	uniform float displacementBias;
#endif`,Om=`#ifdef USE_DISPLACEMENTMAP
	transformed += normalize( objectNormal ) * ( texture2D( displacementMap, vDisplacementMapUv ).x * displacementScale + displacementBias );
#endif`,Bm=`#ifdef USE_EMISSIVEMAP
	vec4 emissiveColor = texture2D( emissiveMap, vEmissiveMapUv );
	#ifdef DECODE_VIDEO_TEXTURE_EMISSIVE
		emissiveColor = sRGBTransferEOTF( emissiveColor );
	#endif
	totalEmissiveRadiance *= emissiveColor.rgb;
#endif`,zm=`#ifdef USE_EMISSIVEMAP
	uniform sampler2D emissiveMap;
#endif`,Hm="gl_FragColor = linearToOutputTexel( gl_FragColor );",Vm=`vec4 LinearTransferOETF( in vec4 value ) {
	return value;
}
vec4 sRGBTransferEOTF( in vec4 value ) {
	return vec4( mix( pow( value.rgb * 0.9478672986 + vec3( 0.0521327014 ), vec3( 2.4 ) ), value.rgb * 0.0773993808, vec3( lessThanEqual( value.rgb, vec3( 0.04045 ) ) ) ), value.a );
}
vec4 sRGBTransferOETF( in vec4 value ) {
	return vec4( mix( pow( value.rgb, vec3( 0.41666 ) ) * 1.055 - vec3( 0.055 ), value.rgb * 12.92, vec3( lessThanEqual( value.rgb, vec3( 0.0031308 ) ) ) ), value.a );
}`,Gm=`#ifdef USE_ENVMAP
	#ifdef ENV_WORLDPOS
		vec3 cameraToFrag;
		if ( isOrthographic ) {
			cameraToFrag = normalize( vec3( - viewMatrix[ 0 ][ 2 ], - viewMatrix[ 1 ][ 2 ], - viewMatrix[ 2 ][ 2 ] ) );
		} else {
			cameraToFrag = normalize( vWorldPosition - cameraPosition );
		}
		vec3 worldNormal = transformNormalByInverseViewMatrix( normal, viewMatrix );
		#ifdef ENVMAP_MODE_REFLECTION
			vec3 reflectVec = reflect( cameraToFrag, worldNormal );
		#else
			vec3 reflectVec = refract( cameraToFrag, worldNormal, refractionRatio );
		#endif
	#else
		vec3 reflectVec = vReflect;
	#endif
	#ifdef ENVMAP_TYPE_CUBE
		vec4 envColor = textureCube( envMap, envMapRotation * reflectVec );
		#ifdef ENVMAP_BLENDING_MULTIPLY
			outgoingLight = mix( outgoingLight, outgoingLight * envColor.xyz, specularStrength * reflectivity );
		#elif defined( ENVMAP_BLENDING_MIX )
			outgoingLight = mix( outgoingLight, envColor.xyz, specularStrength * reflectivity );
		#elif defined( ENVMAP_BLENDING_ADD )
			outgoingLight += envColor.xyz * specularStrength * reflectivity;
		#endif
	#endif
#endif`,Wm=`#ifdef USE_ENVMAP
	uniform float envMapIntensity;
	uniform mat3 envMapRotation;
	#ifdef ENVMAP_TYPE_CUBE
		uniform samplerCube envMap;
	#else
		uniform sampler2D envMap;
	#endif
#endif`,Xm=`#ifdef USE_ENVMAP
	uniform float reflectivity;
	#if defined( USE_BUMPMAP ) || defined( USE_NORMALMAP ) || defined( PHONG ) || defined( LAMBERT )
		#define ENV_WORLDPOS
	#endif
	#ifdef ENV_WORLDPOS
		varying vec3 vWorldPosition;
		uniform float refractionRatio;
	#else
		varying vec3 vReflect;
	#endif
#endif`,qm=`#ifdef USE_ENVMAP
	#if defined( USE_BUMPMAP ) || defined( USE_NORMALMAP ) || defined( PHONG ) || defined( LAMBERT )
		#define ENV_WORLDPOS
	#endif
	#ifdef ENV_WORLDPOS
		
		varying vec3 vWorldPosition;
	#else
		varying vec3 vReflect;
		uniform float refractionRatio;
	#endif
#endif`,$m=`#ifdef USE_ENVMAP
	#ifdef ENV_WORLDPOS
		vWorldPosition = worldPosition.xyz;
	#else
		vec3 cameraToVertex;
		if ( isOrthographic ) {
			cameraToVertex = normalize( vec3( - viewMatrix[ 0 ][ 2 ], - viewMatrix[ 1 ][ 2 ], - viewMatrix[ 2 ][ 2 ] ) );
		} else {
			cameraToVertex = normalize( worldPosition.xyz - cameraPosition );
		}
		vec3 worldNormal = transformNormalByInverseViewMatrix( transformedNormal, viewMatrix );
		#ifdef ENVMAP_MODE_REFLECTION
			vReflect = reflect( cameraToVertex, worldNormal );
		#else
			vReflect = refract( cameraToVertex, worldNormal, refractionRatio );
		#endif
	#endif
#endif`,Ym=`#ifdef USE_FOG
	vFogDepth = - mvPosition.z;
#endif`,Zm=`#ifdef USE_FOG
	varying float vFogDepth;
#endif`,Km=`#ifdef USE_FOG
	#ifdef FOG_EXP2
		float fogFactor = 1.0 - exp( - fogDensity * fogDensity * vFogDepth * vFogDepth );
	#else
		float fogFactor = smoothstep( fogNear, fogFar, vFogDepth );
	#endif
	gl_FragColor.rgb = mix( gl_FragColor.rgb, fogColor, fogFactor );
#endif`,Jm=`#ifdef USE_FOG
	uniform vec3 fogColor;
	varying float vFogDepth;
	#ifdef FOG_EXP2
		uniform float fogDensity;
	#else
		uniform float fogNear;
		uniform float fogFar;
	#endif
#endif`,jm=`#ifdef USE_GRADIENTMAP
	uniform sampler2D gradientMap;
#endif
vec3 getGradientIrradiance( vec3 normal, vec3 lightDirection ) {
	float dotNL = dot( normal, lightDirection );
	vec2 coord = vec2( dotNL * 0.5 + 0.5, 0.0 );
	#ifdef USE_GRADIENTMAP
		return vec3( texture2D( gradientMap, coord ).r );
	#else
		vec2 fw = fwidth( coord ) * 0.5;
		return mix( vec3( 0.7 ), vec3( 1.0 ), smoothstep( 0.7 - fw.x, 0.7 + fw.x, coord.x ) );
	#endif
}`,Qm=`#ifdef USE_LIGHTMAP
	uniform sampler2D lightMap;
	uniform float lightMapIntensity;
#endif`,e0=`LambertMaterial material;
material.diffuseColor = diffuseColor.rgb;
material.specularStrength = specularStrength;`,t0=`varying vec3 vViewPosition;
struct LambertMaterial {
	vec3 diffuseColor;
	float specularStrength;
};
void RE_Direct_Lambert( const in IncidentLight directLight, const in vec3 geometryPosition, const in vec3 geometryNormal, const in vec3 geometryViewDir, const in vec3 geometryClearcoatNormal, const in LambertMaterial material, inout ReflectedLight reflectedLight ) {
	float dotNL = saturate( dot( geometryNormal, directLight.direction ) );
	vec3 irradiance = dotNL * directLight.color;
	reflectedLight.directDiffuse += irradiance * BRDF_Lambert( material.diffuseColor );
}
void RE_IndirectDiffuse_Lambert( const in vec3 irradiance, const in vec3 geometryPosition, const in vec3 geometryNormal, const in vec3 geometryViewDir, const in vec3 geometryClearcoatNormal, const in LambertMaterial material, inout ReflectedLight reflectedLight ) {
	reflectedLight.indirectDiffuse += irradiance * BRDF_Lambert( material.diffuseColor );
}
#define RE_Direct				RE_Direct_Lambert
#define RE_IndirectDiffuse		RE_IndirectDiffuse_Lambert`,n0=`uniform bool receiveShadow;
uniform vec3 ambientLightColor;
#if defined( USE_LIGHT_PROBES )
	uniform vec3 lightProbe[ 9 ];
#endif
vec3 shGetIrradianceAt( in vec3 normal, in vec3 shCoefficients[ 9 ] ) {
	float x = normal.x, y = normal.y, z = normal.z;
	vec3 result = shCoefficients[ 0 ] * 0.886227;
	result += shCoefficients[ 1 ] * 2.0 * 0.511664 * y;
	result += shCoefficients[ 2 ] * 2.0 * 0.511664 * z;
	result += shCoefficients[ 3 ] * 2.0 * 0.511664 * x;
	result += shCoefficients[ 4 ] * 2.0 * 0.429043 * x * y;
	result += shCoefficients[ 5 ] * 2.0 * 0.429043 * y * z;
	result += shCoefficients[ 6 ] * ( 0.743125 * z * z - 0.247708 );
	result += shCoefficients[ 7 ] * 2.0 * 0.429043 * x * z;
	result += shCoefficients[ 8 ] * 0.429043 * ( x * x - y * y );
	return result;
}
vec3 getLightProbeIrradiance( const in vec3 lightProbe[ 9 ], const in vec3 normal ) {
	vec3 worldNormal = transformNormalByInverseViewMatrix( normal, viewMatrix );
	vec3 irradiance = shGetIrradianceAt( worldNormal, lightProbe );
	return irradiance;
}
vec3 getAmbientLightIrradiance( const in vec3 ambientLightColor ) {
	vec3 irradiance = ambientLightColor;
	return irradiance;
}
float getDistanceAttenuation( const in float lightDistance, const in float cutoffDistance, const in float decayExponent ) {
	float distanceFalloff = 1.0 / max( pow( lightDistance, decayExponent ), 0.01 );
	if ( cutoffDistance > 0.0 ) {
		distanceFalloff *= pow2( saturate( 1.0 - pow4( lightDistance / cutoffDistance ) ) );
	}
	return distanceFalloff;
}
float getSpotAttenuation( const in float coneCosine, const in float penumbraCosine, const in float angleCosine ) {
	return smoothstep( coneCosine, penumbraCosine, angleCosine );
}
#if NUM_SUN_LIGHTS > 0
	struct SunLight {
		vec3 direction;
		vec3 color;
	};
	uniform SunLight sunLights[ NUM_SUN_LIGHTS ];
	void getSunLightInfo( const in SunLight sunLight, out IncidentLight light ) {
		light.color = sunLight.color;
		light.direction = sunLight.direction;
		light.visible = true;
	}
#endif
#if NUM_DIR_LIGHTS > 0
	struct DirectionalLight {
		vec3 direction;
		vec3 color;
	};
	uniform DirectionalLight directionalLights[ NUM_DIR_LIGHTS ];
	void getDirectionalLightInfo( const in DirectionalLight directionalLight, out IncidentLight light ) {
		light.color = directionalLight.color;
		light.direction = directionalLight.direction;
		light.visible = true;
	}
#endif
#if NUM_POINT_LIGHTS > 0
	struct PointLight {
		vec3 position;
		vec3 color;
		float distance;
		float decay;
	};
	uniform PointLight pointLights[ NUM_POINT_LIGHTS ];
	void getPointLightInfo( const in PointLight pointLight, const in vec3 geometryPosition, out IncidentLight light ) {
		vec3 lVector = pointLight.position - geometryPosition;
		light.direction = normalize( lVector );
		float lightDistance = length( lVector );
		light.color = pointLight.color;
		light.color *= getDistanceAttenuation( lightDistance, pointLight.distance, pointLight.decay );
		light.visible = ( light.color != vec3( 0.0 ) );
	}
#endif
#if NUM_SPOT_LIGHTS > 0
	struct SpotLight {
		vec3 position;
		vec3 direction;
		vec3 color;
		float distance;
		float decay;
		float coneCos;
		float penumbraCos;
	};
	uniform SpotLight spotLights[ NUM_SPOT_LIGHTS ];
	void getSpotLightInfo( const in SpotLight spotLight, const in vec3 geometryPosition, out IncidentLight light ) {
		vec3 lVector = spotLight.position - geometryPosition;
		light.direction = normalize( lVector );
		float angleCos = dot( light.direction, spotLight.direction );
		float spotAttenuation = getSpotAttenuation( spotLight.coneCos, spotLight.penumbraCos, angleCos );
		if ( spotAttenuation > 0.0 ) {
			float lightDistance = length( lVector );
			light.color = spotLight.color * spotAttenuation;
			light.color *= getDistanceAttenuation( lightDistance, spotLight.distance, spotLight.decay );
			light.visible = ( light.color != vec3( 0.0 ) );
		} else {
			light.color = vec3( 0.0 );
			light.visible = false;
		}
	}
#endif
#if NUM_RECT_AREA_LIGHTS > 0
	struct RectAreaLight {
		vec3 color;
		vec3 position;
		vec3 halfWidth;
		vec3 halfHeight;
	};
	uniform sampler2D ltc_1;	uniform sampler2D ltc_2;
	uniform RectAreaLight rectAreaLights[ NUM_RECT_AREA_LIGHTS ];
#endif
#if NUM_HEMI_LIGHTS > 0
	struct HemisphereLight {
		vec3 direction;
		vec3 skyColor;
		vec3 groundColor;
	};
	uniform HemisphereLight hemisphereLights[ NUM_HEMI_LIGHTS ];
	vec3 getHemisphereLightIrradiance( const in HemisphereLight hemiLight, const in vec3 normal ) {
		float dotNL = dot( normal, hemiLight.direction );
		float hemiDiffuseWeight = 0.5 * dotNL + 0.5;
		vec3 irradiance = mix( hemiLight.groundColor, hemiLight.skyColor, hemiDiffuseWeight );
		return irradiance;
	}
#endif
#include <lightprobes_pars_fragment>`,i0=`#ifdef USE_ENVMAP
	vec3 getIBLIrradiance( const in vec3 normal ) {
		#ifdef ENVMAP_TYPE_CUBE_UV
			vec3 worldNormal = transformNormalByInverseViewMatrix( normal, viewMatrix );
			vec4 envMapColor = textureCubeUV( envMap, envMapRotation * worldNormal, 1.0 );
			return PI * envMapColor.rgb * envMapIntensity;
		#else
			return vec3( 0.0 );
		#endif
	}
	vec3 getIBLRadiance( const in vec3 viewDir, const in vec3 normal, const in float roughness ) {
		#ifdef ENVMAP_TYPE_CUBE_UV
			vec3 reflectVec = reflect( - viewDir, normal );
			reflectVec = normalize( mix( reflectVec, normal, pow4( roughness ) ) );
			reflectVec = transformDirectionByInverseViewMatrix( reflectVec, viewMatrix );
			vec4 envMapColor = textureCubeUV( envMap, envMapRotation * reflectVec, roughness );
			return envMapColor.rgb * envMapIntensity;
		#else
			return vec3( 0.0 );
		#endif
	}
	#ifdef USE_RETROREFLECTION
		vec3 getIBLRetroRadiance( const in vec3 viewDir, const in vec3 normal, const in float roughness ) {
			#ifdef ENVMAP_TYPE_CUBE_UV
				vec3 retroVec = normalize( mix( viewDir, normal, pow4( roughness ) ) );
				retroVec = transformDirectionByInverseViewMatrix( retroVec, viewMatrix );
				vec4 envMapColor = textureCubeUV( envMap, envMapRotation * retroVec, roughness );
				return envMapColor.rgb * envMapIntensity;
			#else
				return vec3( 0.0 );
			#endif
		}
	#endif
	#ifdef USE_ANISOTROPY
		vec3 getIBLAnisotropyRadiance( const in vec3 viewDir, const in vec3 normal, const in float roughness, const in vec3 bitangent, const in float anisotropy ) {
			#ifdef ENVMAP_TYPE_CUBE_UV
				vec3 bentNormal = cross( bitangent, viewDir );
				bentNormal = normalize( cross( bentNormal, bitangent ) );
				bentNormal = normalize( mix( bentNormal, normal, pow2( pow2( 1.0 - anisotropy * ( 1.0 - roughness ) ) ) ) );
				return getIBLRadiance( viewDir, bentNormal, roughness );
			#else
				return vec3( 0.0 );
			#endif
		}
		#ifdef USE_RETROREFLECTION
			vec3 getIBLAnisotropyRetroRadiance( const in vec3 viewDir, const in vec3 normal, const in float roughness, const in vec3 bitangent, const in float anisotropy ) {
				#ifdef ENVMAP_TYPE_CUBE_UV
					vec3 bentNormal = cross( bitangent, viewDir );
					bentNormal = normalize( cross( bentNormal, bitangent ) );
					bentNormal = normalize( mix( bentNormal, normal, pow2( pow2( 1.0 - anisotropy * ( 1.0 - roughness ) ) ) ) );
					return getIBLRetroRadiance( viewDir, bentNormal, roughness );
				#else
					return vec3( 0.0 );
				#endif
			}
		#endif
	#endif
#endif`,r0=`ToonMaterial material;
material.diffuseColor = diffuseColor.rgb;`,s0=`varying vec3 vViewPosition;
struct ToonMaterial {
	vec3 diffuseColor;
};
void RE_Direct_Toon( const in IncidentLight directLight, const in vec3 geometryPosition, const in vec3 geometryNormal, const in vec3 geometryViewDir, const in vec3 geometryClearcoatNormal, const in ToonMaterial material, inout ReflectedLight reflectedLight ) {
	vec3 irradiance = getGradientIrradiance( geometryNormal, directLight.direction ) * directLight.color;
	reflectedLight.directDiffuse += irradiance * BRDF_Lambert( material.diffuseColor );
}
void RE_IndirectDiffuse_Toon( const in vec3 irradiance, const in vec3 geometryPosition, const in vec3 geometryNormal, const in vec3 geometryViewDir, const in vec3 geometryClearcoatNormal, const in ToonMaterial material, inout ReflectedLight reflectedLight ) {
	reflectedLight.indirectDiffuse += irradiance * BRDF_Lambert( material.diffuseColor );
}
#define RE_Direct				RE_Direct_Toon
#define RE_IndirectDiffuse		RE_IndirectDiffuse_Toon`,o0=`BlinnPhongMaterial material;
material.diffuseColor = diffuseColor.rgb;
material.specularColor = specular;
material.specularShininess = shininess;
material.specularStrength = specularStrength;`,a0=`varying vec3 vViewPosition;
struct BlinnPhongMaterial {
	vec3 diffuseColor;
	vec3 specularColor;
	float specularShininess;
	float specularStrength;
};
void RE_Direct_BlinnPhong( const in IncidentLight directLight, const in vec3 geometryPosition, const in vec3 geometryNormal, const in vec3 geometryViewDir, const in vec3 geometryClearcoatNormal, const in BlinnPhongMaterial material, inout ReflectedLight reflectedLight ) {
	float dotNL = saturate( dot( geometryNormal, directLight.direction ) );
	vec3 irradiance = dotNL * directLight.color;
	reflectedLight.directDiffuse += irradiance * BRDF_Lambert( material.diffuseColor );
	reflectedLight.directSpecular += irradiance * BRDF_BlinnPhong( directLight.direction, geometryViewDir, geometryNormal, material.specularColor, material.specularShininess ) * material.specularStrength;
}
void RE_IndirectDiffuse_BlinnPhong( const in vec3 irradiance, const in vec3 geometryPosition, const in vec3 geometryNormal, const in vec3 geometryViewDir, const in vec3 geometryClearcoatNormal, const in BlinnPhongMaterial material, inout ReflectedLight reflectedLight ) {
	reflectedLight.indirectDiffuse += irradiance * BRDF_Lambert( material.diffuseColor );
}
#define RE_Direct				RE_Direct_BlinnPhong
#define RE_IndirectDiffuse		RE_IndirectDiffuse_BlinnPhong`,l0=`PhysicalMaterial material;
material.diffuseColor = diffuseColor.rgb;
material.diffuseContribution = diffuseColor.rgb * ( 1.0 - metalnessFactor );
material.metalness = metalnessFactor;
vec3 dxy = max( abs( dFdx( nonPerturbedNormal ) ), abs( dFdy( nonPerturbedNormal ) ) );
float geometryRoughness = max( max( dxy.x, dxy.y ), dxy.z );
material.roughness = max( roughnessFactor, 0.0525 );material.roughness += geometryRoughness;
material.roughness = min( material.roughness, 1.0 );
#ifdef IOR
	material.ior = ior;
	#ifdef USE_SPECULAR
		float specularIntensityFactor = specularIntensity;
		vec3 specularColorFactor = specularColor;
		#ifdef USE_SPECULAR_COLORMAP
			specularColorFactor *= texture2D( specularColorMap, vSpecularColorMapUv ).rgb;
		#endif
		#ifdef USE_SPECULAR_INTENSITYMAP
			specularIntensityFactor *= texture2D( specularIntensityMap, vSpecularIntensityMapUv ).a;
		#endif
		material.specularF90 = mix( specularIntensityFactor, 1.0, metalnessFactor );
	#else
		float specularIntensityFactor = 1.0;
		vec3 specularColorFactor = vec3( 1.0 );
		material.specularF90 = 1.0;
	#endif
	material.specularColor = min( pow2( ( material.ior - 1.0 ) / ( material.ior + 1.0 ) ) * specularColorFactor, vec3( 1.0 ) ) * specularIntensityFactor;
	material.specularColorBlended = mix( material.specularColor, diffuseColor.rgb, metalnessFactor );
#else
	material.specularColor = vec3( 0.04 );
	material.specularColorBlended = mix( material.specularColor, diffuseColor.rgb, metalnessFactor );
	material.specularF90 = 1.0;
#endif
#ifdef USE_CLEARCOAT
	material.clearcoat = clearcoat;
	material.clearcoatRoughness = clearcoatRoughness;
	material.clearcoatF0 = vec3( 0.04 );
	material.clearcoatF90 = 1.0;
	#ifdef USE_CLEARCOATMAP
		material.clearcoat *= texture2D( clearcoatMap, vClearcoatMapUv ).x;
	#endif
	#ifdef USE_CLEARCOAT_ROUGHNESSMAP
		material.clearcoatRoughness *= texture2D( clearcoatRoughnessMap, vClearcoatRoughnessMapUv ).y;
	#endif
	material.clearcoat = saturate( material.clearcoat );	material.clearcoatRoughness = max( material.clearcoatRoughness, 0.0525 );
	material.clearcoatRoughness += geometryRoughness;
	material.clearcoatRoughness = min( material.clearcoatRoughness, 1.0 );
#endif
#ifdef USE_DISPERSION
	material.dispersion = dispersion;
#endif
#ifdef USE_RETROREFLECTION
	material.retroreflectivity = retroreflectivity;
#endif
#ifdef USE_IRIDESCENCE
	material.iridescence = iridescence;
	material.iridescenceIOR = iridescenceIOR;
	#ifdef USE_IRIDESCENCEMAP
		material.iridescence *= texture2D( iridescenceMap, vIridescenceMapUv ).r;
	#endif
	#ifdef USE_IRIDESCENCE_THICKNESSMAP
		material.iridescenceThickness = (iridescenceThicknessMaximum - iridescenceThicknessMinimum) * texture2D( iridescenceThicknessMap, vIridescenceThicknessMapUv ).g + iridescenceThicknessMinimum;
	#else
		material.iridescenceThickness = iridescenceThicknessMaximum;
	#endif
#endif
#ifdef USE_SHEEN
	material.sheenColor = sheenColor;
	#ifdef USE_SHEEN_COLORMAP
		material.sheenColor *= texture2D( sheenColorMap, vSheenColorMapUv ).rgb;
	#endif
	material.sheenRoughness = clamp( sheenRoughness, 0.0001, 1.0 );
	#ifdef USE_SHEEN_ROUGHNESSMAP
		material.sheenRoughness *= texture2D( sheenRoughnessMap, vSheenRoughnessMapUv ).a;
	#endif
#endif
#ifdef USE_ANISOTROPY
	#ifdef USE_ANISOTROPYMAP
		mat2 anisotropyMat = mat2( anisotropyVector.x, anisotropyVector.y, - anisotropyVector.y, anisotropyVector.x );
		vec3 anisotropyPolar = texture2D( anisotropyMap, vAnisotropyMapUv ).rgb;
		vec2 anisotropyV = anisotropyMat * normalize( 2.0 * anisotropyPolar.rg - vec2( 1.0 ) ) * anisotropyPolar.b;
	#else
		vec2 anisotropyV = anisotropyVector;
	#endif
	material.anisotropy = length( anisotropyV );
	if( material.anisotropy == 0.0 ) {
		anisotropyV = vec2( 1.0, 0.0 );
	} else {
		anisotropyV /= material.anisotropy;
		material.anisotropy = saturate( material.anisotropy );
	}
	material.alphaT = mix( pow2( material.roughness ), 1.0, pow2( material.anisotropy ) );
	material.anisotropyT = tbn[ 0 ] * anisotropyV.x + tbn[ 1 ] * anisotropyV.y;
	material.anisotropyB = tbn[ 1 ] * anisotropyV.x - tbn[ 0 ] * anisotropyV.y;
#endif`,c0=`uniform sampler2D dfgLUT;
struct PhysicalMaterial {
	vec3 diffuseColor;
	vec3 diffuseContribution;
	vec3 specularColor;
	vec3 specularColorBlended;
	float roughness;
	float metalness;
	float specularF90;
	float dispersion;
	vec2 dfg;
	vec3 multiScatteringCompensation;
	#ifdef USE_RETROREFLECTION
		float retroreflectivity;
	#endif
	#ifdef USE_CLEARCOAT
		float clearcoat;
		float clearcoatRoughness;
		vec3 clearcoatF0;
		float clearcoatF90;
	#endif
	#ifdef USE_IRIDESCENCE
		float iridescence;
		float iridescenceIOR;
		float iridescenceThickness;
		vec3 iridescenceFresnel;
		vec3 iridescenceF0Dielectric;
		vec3 iridescenceF0Metallic;
	#endif
	#ifdef USE_SHEEN
		vec3 sheenColor;
		float sheenRoughness;
	#endif
	#ifdef IOR
		float ior;
	#endif
	#ifdef USE_TRANSMISSION
		float transmission;
		float transmissionAlpha;
		float thickness;
		float attenuationDistance;
		vec3 attenuationColor;
	#endif
	#ifdef USE_ANISOTROPY
		float anisotropy;
		float alphaT;
		vec3 anisotropyT;
		vec3 anisotropyB;
	#endif
};
vec3 clearcoatSpecularDirect = vec3( 0.0 );
vec3 clearcoatSpecularIndirect = vec3( 0.0 );
vec3 sheenSpecularDirect = vec3( 0.0 );
vec3 sheenSpecularIndirect = vec3(0.0 );
vec3 Schlick_to_F0( const in vec3 f, const in float f90, const in float dotVH ) {
    float x = clamp( 1.0 - dotVH, 0.0, 1.0 );
    float x2 = x * x;
    float x5 = clamp( x * x2 * x2, 0.0, 0.9999 );
    return ( f - vec3( f90 ) * x5 ) / ( 1.0 - x5 );
}
float V_GGX_SmithCorrelated( const in float alpha, const in float dotNL, const in float dotNV ) {
	float a2 = pow2( alpha );
	float gv = dotNL * sqrt( a2 + ( 1.0 - a2 ) * pow2( dotNV ) );
	float gl = dotNV * sqrt( a2 + ( 1.0 - a2 ) * pow2( dotNL ) );
	return 0.5 / max( gv + gl, EPSILON );
}
float D_GGX( const in float alpha, const in float dotNH ) {
	float a2 = pow2( alpha );
	float denom = pow2( dotNH ) * ( a2 - 1.0 ) + 1.0;
	return RECIPROCAL_PI * a2 / pow2( denom );
}
#ifdef USE_ANISOTROPY
	float V_GGX_SmithCorrelated_Anisotropic( const in float alphaT, const in float alphaB, const in float dotTV, const in float dotBV, const in float dotTL, const in float dotBL, const in float dotNV, const in float dotNL ) {
		float gv = dotNL * length( vec3( alphaT * dotTV, alphaB * dotBV, dotNV ) );
		float gl = dotNV * length( vec3( alphaT * dotTL, alphaB * dotBL, dotNL ) );
		return 0.5 / max( gv + gl, EPSILON );
	}
	float D_GGX_Anisotropic( const in float alphaT, const in float alphaB, const in float dotNH, const in float dotTH, const in float dotBH ) {
		float a2 = alphaT * alphaB;
		highp vec3 v = vec3( alphaB * dotTH, alphaT * dotBH, a2 * dotNH );
		highp float v2 = dot( v, v );
		float w2 = a2 / v2;
		return RECIPROCAL_PI * a2 * pow2 ( w2 );
	}
#endif
#ifdef USE_CLEARCOAT
	vec3 BRDF_GGX_Clearcoat( const in vec3 lightDir, const in vec3 viewDir, const in vec3 normal, const in PhysicalMaterial material) {
		vec3 f0 = material.clearcoatF0;
		float f90 = material.clearcoatF90;
		float roughness = material.clearcoatRoughness;
		float alpha = pow2( roughness );
		vec3 halfDir = normalize( lightDir + viewDir );
		float dotNL = saturate( dot( normal, lightDir ) );
		float dotNV = saturate( dot( normal, viewDir ) );
		float dotNH = saturate( dot( normal, halfDir ) );
		float dotVH = saturate( dot( viewDir, halfDir ) );
		vec3 F = F_Schlick( f0, f90, dotVH );
		float V = V_GGX_SmithCorrelated( alpha, dotNL, dotNV );
		float D = D_GGX( alpha, dotNH );
		return F * ( V * D );
	}
#endif
vec3 BRDF_GGX( const in vec3 lightDir, const in vec3 viewDir, const in vec3 normal, const in PhysicalMaterial material ) {
	vec3 f0 = material.specularColorBlended;
	float f90 = material.specularF90;
	float roughness = material.roughness;
	float alpha = pow2( roughness );
	vec3 halfDir = normalize( lightDir + viewDir );
	float dotNL = saturate( dot( normal, lightDir ) );
	float dotNV = saturate( dot( normal, viewDir ) );
	float dotNH = saturate( dot( normal, halfDir ) );
	float dotVH = saturate( dot( viewDir, halfDir ) );
	vec3 F = F_Schlick( f0, f90, dotVH );
	#ifdef USE_IRIDESCENCE
		F = mix( F, material.iridescenceFresnel, material.iridescence );
	#endif
	#ifdef USE_ANISOTROPY
		float dotTL = dot( material.anisotropyT, lightDir );
		float dotTV = dot( material.anisotropyT, viewDir );
		float dotTH = dot( material.anisotropyT, halfDir );
		float dotBL = dot( material.anisotropyB, lightDir );
		float dotBV = dot( material.anisotropyB, viewDir );
		float dotBH = dot( material.anisotropyB, halfDir );
		float V = V_GGX_SmithCorrelated_Anisotropic( material.alphaT, alpha, dotTV, dotBV, dotTL, dotBL, dotNV, dotNL );
		float D = D_GGX_Anisotropic( material.alphaT, alpha, dotNH, dotTH, dotBH );
	#else
		float V = V_GGX_SmithCorrelated( alpha, dotNL, dotNV );
		float D = D_GGX( alpha, dotNH );
	#endif
	return F * ( V * D );
}
vec2 LTC_Uv( const in vec3 N, const in vec3 V, const in float roughness ) {
	const float LUT_SIZE = 64.0;
	const float LUT_SCALE = ( LUT_SIZE - 1.0 ) / LUT_SIZE;
	const float LUT_BIAS = 0.5 / LUT_SIZE;
	float dotNV = saturate( dot( N, V ) );
	vec2 uv = vec2( roughness, sqrt( 1.0 - dotNV ) );
	uv = uv * LUT_SCALE + LUT_BIAS;
	return uv;
}
float LTC_ClippedSphereFormFactor( const in vec3 f ) {
	float l = length( f );
	return max( ( l * l + f.z ) / ( l + 1.0 ), 0.0 );
}
vec3 LTC_EdgeVectorFormFactor( const in vec3 v1, const in vec3 v2 ) {
	float x = dot( v1, v2 );
	float y = abs( x );
	float a = 0.8543985 + ( 0.4965155 + 0.0145206 * y ) * y;
	float b = 3.4175940 + ( 4.1616724 + y ) * y;
	float v = a / b;
	float theta_sintheta = ( x > 0.0 ) ? v : 0.5 * inversesqrt( max( 1.0 - x * x, 1e-7 ) ) - v;
	return cross( v1, v2 ) * theta_sintheta;
}
vec3 LTC_Evaluate( const in vec3 N, const in vec3 V, const in vec3 P, const in mat3 mInv, const in vec3 rectCoords[ 4 ] ) {
	vec3 v1 = rectCoords[ 1 ] - rectCoords[ 0 ];
	vec3 v2 = rectCoords[ 3 ] - rectCoords[ 0 ];
	vec3 lightNormal = cross( v1, v2 );
	if( dot( lightNormal, P - rectCoords[ 0 ] ) < 0.0 ) return vec3( 0.0 );
	vec3 T1, T2;
	T1 = normalize( V - N * dot( V, N ) );
	T2 = - cross( N, T1 );
	mat3 mat = mInv * transpose( mat3( T1, T2, N ) );
	vec3 coords[ 4 ];
	coords[ 0 ] = mat * ( rectCoords[ 0 ] - P );
	coords[ 1 ] = mat * ( rectCoords[ 1 ] - P );
	coords[ 2 ] = mat * ( rectCoords[ 2 ] - P );
	coords[ 3 ] = mat * ( rectCoords[ 3 ] - P );
	coords[ 0 ] = normalize( coords[ 0 ] );
	coords[ 1 ] = normalize( coords[ 1 ] );
	coords[ 2 ] = normalize( coords[ 2 ] );
	coords[ 3 ] = normalize( coords[ 3 ] );
	vec3 vectorFormFactor = vec3( 0.0 );
	vectorFormFactor += LTC_EdgeVectorFormFactor( coords[ 0 ], coords[ 1 ] );
	vectorFormFactor += LTC_EdgeVectorFormFactor( coords[ 1 ], coords[ 2 ] );
	vectorFormFactor += LTC_EdgeVectorFormFactor( coords[ 2 ], coords[ 3 ] );
	vectorFormFactor += LTC_EdgeVectorFormFactor( coords[ 3 ], coords[ 0 ] );
	float result = LTC_ClippedSphereFormFactor( vectorFormFactor );
	return vec3( result );
}
#if defined( USE_SHEEN )
float D_Charlie( float roughness, float dotNH ) {
	float alpha = pow2( roughness );
	float invAlpha = 1.0 / alpha;
	float cos2h = dotNH * dotNH;
	float sin2h = max( 1.0 - cos2h, 0.0078125 );
	return ( 2.0 + invAlpha ) * pow( sin2h, invAlpha * 0.5 ) / ( 2.0 * PI );
}
float V_Neubelt( float dotNV, float dotNL ) {
	return saturate( 1.0 / ( 4.0 * ( dotNL + dotNV - dotNL * dotNV ) ) );
}
vec3 BRDF_Sheen( const in vec3 lightDir, const in vec3 viewDir, const in vec3 normal, vec3 sheenColor, const in float sheenRoughness ) {
	vec3 halfDir = normalize( lightDir + viewDir );
	float dotNL = saturate( dot( normal, lightDir ) );
	float dotNV = saturate( dot( normal, viewDir ) );
	float dotNH = saturate( dot( normal, halfDir ) );
	float D = D_Charlie( sheenRoughness, dotNH );
	float V = V_Neubelt( dotNV, dotNL );
	return sheenColor * ( D * V );
}
#endif
float IBLSheenBRDF( const in vec3 normal, const in vec3 viewDir, const in float roughness ) {
	float dotNV = saturate( dot( normal, viewDir ) );
	float r2 = roughness * roughness;
	float rInv = 1.0 / ( roughness + 0.1 );
	float a = -1.9362 + 1.0678 * roughness + 0.4573 * r2 - 0.8469 * rInv;
	float b = -0.6014 + 0.5538 * roughness - 0.4670 * r2 - 0.1255 * rInv;
	float DG = exp( a * dotNV + b );
	return saturate( DG );
}
vec3 EnvironmentBRDF( const in vec3 normal, const in vec3 viewDir, const in vec3 specularColor, const in float specularF90, const in float roughness ) {
	float dotNV = saturate( dot( normal, viewDir ) );
	vec2 fab = texture2D( dfgLUT, vec2( roughness, dotNV ) ).rg;
	return specularColor * fab.x + specularF90 * fab.y;
}
#ifdef USE_IRIDESCENCE
void computeMultiscatteringIridescence( const in vec2 fab, const in vec3 specularColor, const in float specularF90, const in float iridescence, const in vec3 iridescenceF0, inout vec3 singleScatter, inout vec3 multiScatter ) {
#else
void computeMultiscattering( const in vec2 fab, const in vec3 specularColor, const in float specularF90, inout vec3 singleScatter, inout vec3 multiScatter ) {
#endif
	#ifdef USE_IRIDESCENCE
		vec3 Fr = mix( specularColor, iridescenceF0, iridescence );
	#else
		vec3 Fr = specularColor;
	#endif
	vec3 FssEss = Fr * fab.x + specularF90 * fab.y;
	float Ess = fab.x + fab.y;
	float Ems = 1.0 - Ess;
	vec3 Favg = Fr + ( 1.0 - Fr ) * 0.047619;	vec3 Fms = FssEss * Favg / ( 1.0 - Ems * Favg );
	singleScatter += FssEss;
	multiScatter += Fms * Ems;
}
#if NUM_RECT_AREA_LIGHTS > 0
	void RE_Direct_RectArea_Physical( const in RectAreaLight rectAreaLight, const in vec3 geometryPosition, const in vec3 geometryNormal, const in vec3 geometryViewDir, const in vec3 geometryClearcoatNormal, const in PhysicalMaterial material, inout ReflectedLight reflectedLight ) {
		vec3 normal = geometryNormal;
		vec3 viewDir = geometryViewDir;
		vec3 position = geometryPosition;
		vec3 lightPos = rectAreaLight.position;
		vec3 halfWidth = rectAreaLight.halfWidth;
		vec3 halfHeight = rectAreaLight.halfHeight;
		vec3 lightColor = rectAreaLight.color;
		float roughness = material.roughness;
		vec3 rectCoords[ 4 ];
		rectCoords[ 0 ] = lightPos + halfWidth - halfHeight;		rectCoords[ 1 ] = lightPos - halfWidth - halfHeight;
		rectCoords[ 2 ] = lightPos - halfWidth + halfHeight;
		rectCoords[ 3 ] = lightPos + halfWidth + halfHeight;
		vec2 uv = LTC_Uv( normal, viewDir, roughness );
		vec4 t1 = texture2D( ltc_1, uv );
		vec4 t2 = texture2D( ltc_2, uv );
		mat3 mInv = mat3(
			vec3( t1.x, 0, t1.y ),
			vec3(    0, 1,    0 ),
			vec3( t1.z, 0, t1.w )
		);
		vec3 fresnel = ( material.specularColorBlended * t2.x + ( material.specularF90 - material.specularColorBlended ) * t2.y );
		reflectedLight.directSpecular += lightColor * fresnel * LTC_Evaluate( normal, viewDir, position, mInv, rectCoords );
		reflectedLight.directDiffuse += lightColor * material.diffuseContribution * LTC_Evaluate( normal, viewDir, position, mat3( 1.0 ), rectCoords );
		#ifdef USE_CLEARCOAT
			vec3 Ncc = geometryClearcoatNormal;
			vec2 uvClearcoat = LTC_Uv( Ncc, viewDir, material.clearcoatRoughness );
			vec4 t1Clearcoat = texture2D( ltc_1, uvClearcoat );
			vec4 t2Clearcoat = texture2D( ltc_2, uvClearcoat );
			mat3 mInvClearcoat = mat3(
				vec3( t1Clearcoat.x, 0, t1Clearcoat.y ),
				vec3(             0, 1,             0 ),
				vec3( t1Clearcoat.z, 0, t1Clearcoat.w )
			);
			vec3 fresnelClearcoat = material.clearcoatF0 * t2Clearcoat.x + ( material.clearcoatF90 - material.clearcoatF0 ) * t2Clearcoat.y;
			clearcoatSpecularDirect += lightColor * fresnelClearcoat * LTC_Evaluate( Ncc, viewDir, position, mInvClearcoat, rectCoords );
		#endif
	}
#endif
void RE_Direct_Physical( const in IncidentLight directLight, const in vec3 geometryPosition, const in vec3 geometryNormal, const in vec3 geometryViewDir, const in vec3 geometryClearcoatNormal, const in PhysicalMaterial material, inout ReflectedLight reflectedLight ) {
	float dotNL = saturate( dot( geometryNormal, directLight.direction ) );
	vec3 irradiance = dotNL * directLight.color;
	#ifdef USE_CLEARCOAT
		float dotNLcc = saturate( dot( geometryClearcoatNormal, directLight.direction ) );
		vec3 ccIrradiance = dotNLcc * directLight.color;
		clearcoatSpecularDirect += ccIrradiance * BRDF_GGX_Clearcoat( directLight.direction, geometryViewDir, geometryClearcoatNormal, material );
	#endif
	#ifdef USE_SHEEN
 
 		sheenSpecularDirect += irradiance * BRDF_Sheen( directLight.direction, geometryViewDir, geometryNormal, material.sheenColor, material.sheenRoughness );
 
 		float sheenAlbedoV = IBLSheenBRDF( geometryNormal, geometryViewDir, material.sheenRoughness );
 		float sheenAlbedoL = IBLSheenBRDF( geometryNormal, directLight.direction, material.sheenRoughness );
 
 		float sheenEnergyComp = 1.0 - max3( material.sheenColor ) * max( sheenAlbedoV, sheenAlbedoL );
 
 		irradiance *= sheenEnergyComp;
 
 	#endif
	vec3 specularBRDF = BRDF_GGX( directLight.direction, geometryViewDir, geometryNormal, material );
	#ifdef USE_RETROREFLECTION
		vec3 retroViewDir = reflect( - geometryViewDir, geometryNormal );
		vec3 retroSpecularBRDF = BRDF_GGX( directLight.direction, retroViewDir, geometryNormal, material );
		specularBRDF = mix( specularBRDF, retroSpecularBRDF, saturate( material.retroreflectivity ) );
	#endif
	reflectedLight.directSpecular += irradiance * specularBRDF * material.multiScatteringCompensation;
	vec3 halfDir = normalize( directLight.direction + geometryViewDir );
	float dotVH = saturate( dot( geometryViewDir, halfDir ) );
	vec3 F = F_Schlick( material.specularColor, material.specularF90, dotVH );
	#ifdef USE_RETROREFLECTION
		vec3 retroHalfDir = normalize( directLight.direction + retroViewDir );
		float dotRetroVH = saturate( dot( retroViewDir, retroHalfDir ) );
		vec3 retroF = F_Schlick( material.specularColor, material.specularF90, dotRetroVH );
		F = mix( F, retroF, saturate( material.retroreflectivity ) );
	#endif
	reflectedLight.directDiffuse += irradiance * BRDF_Lambert( material.diffuseContribution ) * ( 1.0 - F );
}
void RE_IndirectDiffuse_Physical( const in vec3 irradiance, const in vec3 geometryPosition, const in vec3 geometryNormal, const in vec3 geometryViewDir, const in vec3 geometryClearcoatNormal, const in PhysicalMaterial material, inout ReflectedLight reflectedLight ) {
	vec3 singleScattering = vec3( 0.0 );
	vec3 multiScattering = vec3( 0.0 );
	#ifdef USE_IRIDESCENCE
		computeMultiscatteringIridescence( material.dfg, material.specularColor, material.specularF90, material.iridescence, material.iridescenceF0Dielectric, singleScattering, multiScattering );
	#else
		computeMultiscattering( material.dfg, material.specularColor, material.specularF90, singleScattering, multiScattering );
	#endif
	vec3 diffuse = irradiance * BRDF_Lambert( material.diffuseContribution ) * ( 1.0 - singleScattering - multiScattering );
	#ifdef USE_SHEEN
		float sheenAlbedo = IBLSheenBRDF( geometryNormal, geometryViewDir, material.sheenRoughness );
		sheenSpecularIndirect += irradiance * material.sheenColor * sheenAlbedo * RECIPROCAL_PI;
		float sheenEnergyComp = 1.0 - max3( material.sheenColor ) * sheenAlbedo;
		diffuse *= sheenEnergyComp;
	#endif
	reflectedLight.indirectDiffuse += diffuse;
}
void RE_IndirectSpecular_Physical( const in vec3 radiance, const in vec3 irradiance, const in vec3 clearcoatRadiance, const in vec3 geometryPosition, const in vec3 geometryNormal, const in vec3 geometryViewDir, const in vec3 geometryClearcoatNormal, const in PhysicalMaterial material, inout ReflectedLight reflectedLight) {
	#ifdef USE_CLEARCOAT
		clearcoatSpecularIndirect += clearcoatRadiance * EnvironmentBRDF( geometryClearcoatNormal, geometryViewDir, material.clearcoatF0, material.clearcoatF90, material.clearcoatRoughness );
	#endif
	#ifdef USE_SHEEN
		sheenSpecularIndirect += irradiance * material.sheenColor * IBLSheenBRDF( geometryNormal, geometryViewDir, material.sheenRoughness ) * RECIPROCAL_PI;
 	#endif
	vec3 singleScatteringDielectric = vec3( 0.0 );
	vec3 multiScatteringDielectric = vec3( 0.0 );
	vec3 singleScatteringMetallic = vec3( 0.0 );
	vec3 multiScatteringMetallic = vec3( 0.0 );
	#ifdef USE_IRIDESCENCE
		computeMultiscatteringIridescence( material.dfg, material.specularColor, material.specularF90, material.iridescence, material.iridescenceF0Dielectric, singleScatteringDielectric, multiScatteringDielectric );
		computeMultiscatteringIridescence( material.dfg, material.diffuseColor, material.specularF90, material.iridescence, material.iridescenceF0Metallic, singleScatteringMetallic, multiScatteringMetallic );
	#else
		computeMultiscattering( material.dfg, material.specularColor, material.specularF90, singleScatteringDielectric, multiScatteringDielectric );
		computeMultiscattering( material.dfg, material.diffuseColor, material.specularF90, singleScatteringMetallic, multiScatteringMetallic );
	#endif
	vec3 singleScattering = mix( singleScatteringDielectric, singleScatteringMetallic, material.metalness );
	vec3 multiScattering = mix( multiScatteringDielectric, multiScatteringMetallic, material.metalness );
	vec3 totalScatteringDielectric = singleScatteringDielectric + multiScatteringDielectric;
	vec3 diffuse = material.diffuseContribution * ( 1.0 - totalScatteringDielectric );
	vec3 cosineWeightedIrradiance = irradiance * RECIPROCAL_PI;
	vec3 indirectSpecular = radiance * singleScattering;
	indirectSpecular += multiScattering * cosineWeightedIrradiance;
	vec3 indirectDiffuse = diffuse * cosineWeightedIrradiance;
	#ifdef USE_SHEEN
		float sheenAlbedo = IBLSheenBRDF( geometryNormal, geometryViewDir, material.sheenRoughness );
		float sheenEnergyComp = 1.0 - max3( material.sheenColor ) * sheenAlbedo;
		indirectSpecular *= sheenEnergyComp;
		indirectDiffuse *= sheenEnergyComp;
	#endif
	reflectedLight.indirectSpecular += indirectSpecular;
	reflectedLight.indirectDiffuse += indirectDiffuse;
}
#define RE_Direct				RE_Direct_Physical
#define RE_Direct_RectArea		RE_Direct_RectArea_Physical
#define RE_IndirectDiffuse		RE_IndirectDiffuse_Physical
#define RE_IndirectSpecular		RE_IndirectSpecular_Physical
float computeSpecularOcclusion( const in float dotNV, const in float ambientOcclusion, const in float roughness ) {
	return saturate( pow( dotNV + ambientOcclusion, exp2( - 16.0 * roughness - 1.0 ) ) - 1.0 + ambientOcclusion );
}`,u0=`
vec3 geometryPosition = - vViewPosition;
vec3 geometryNormal = normal;
vec3 geometryViewDir = ( isOrthographic ) ? vec3( 0, 0, 1 ) : normalize( vViewPosition );
vec3 geometryClearcoatNormal = vec3( 0.0 );
#ifdef USE_CLEARCOAT
	geometryClearcoatNormal = clearcoatNormal;
#endif
#ifdef USE_IRIDESCENCE
	float dotNVi = saturate( dot( normal, geometryViewDir ) );
	if ( material.iridescenceThickness == 0.0 ) {
		material.iridescence = 0.0;
	} else {
		material.iridescence = saturate( material.iridescence );
	}
	if ( material.iridescence > 0.0 ) {
		vec3 iridescenceFresnelDielectric = evalIridescence( 1.0, material.iridescenceIOR, dotNVi, material.iridescenceThickness, material.specularColor );
		vec3 iridescenceFresnelMetallic = evalIridescence( 1.0, material.iridescenceIOR, dotNVi, material.iridescenceThickness, material.diffuseColor );
		material.iridescenceFresnel = mix( iridescenceFresnelDielectric, iridescenceFresnelMetallic, material.metalness );
		material.iridescenceF0Dielectric = Schlick_to_F0( iridescenceFresnelDielectric, 1.0, dotNVi );
		material.iridescenceF0Metallic = Schlick_to_F0( iridescenceFresnelMetallic, 1.0, dotNVi );
	}
#endif
#ifdef STANDARD
	float dotNVms = saturate( dot( geometryNormal, geometryViewDir ) );
	material.dfg = texture2D( dfgLUT, vec2( material.roughness, dotNVms ) ).rg;
	#if ( NUM_SUN_LIGHTS > 0 || NUM_DIR_LIGHTS > 0 || NUM_POINT_LIGHTS > 0 || NUM_SPOT_LIGHTS > 0 )
		float EssMs = material.dfg.x + material.dfg.y;
		material.multiScatteringCompensation = 1.0 + material.specularColorBlended * ( 1.0 / EssMs - 1.0 );
	#endif
#endif
IncidentLight directLight;
#if ( NUM_POINT_LIGHTS > 0 ) && defined( RE_Direct )
	PointLight pointLight;
	#if defined( USE_SHADOWMAP ) && NUM_POINT_LIGHT_SHADOWS > 0
	PointLightShadow pointLightShadow;
	#endif
	#pragma unroll_loop_start
	for ( int i = 0; i < NUM_POINT_LIGHTS; i ++ ) {
		pointLight = pointLights[ i ];
		getPointLightInfo( pointLight, geometryPosition, directLight );
		#if defined( USE_SHADOWMAP ) && ( UNROLLED_LOOP_INDEX < NUM_POINT_LIGHT_SHADOWS ) && ( defined( SHADOWMAP_TYPE_PCF ) || defined( SHADOWMAP_TYPE_BASIC ) )
		pointLightShadow = pointLightShadows[ i ];
		directLight.color *= ( directLight.visible && receiveShadow ) ? getPointShadow( pointShadowMap[ i ], pointLightShadow.shadowMapSize, pointLightShadow.shadowIntensity, pointLightShadow.shadowBias, pointLightShadow.shadowRadius, vPointShadowCoord[ i ], pointLightShadow.shadowCameraNear, pointLightShadow.shadowCameraFar ) : 1.0;
		#endif
		RE_Direct( directLight, geometryPosition, geometryNormal, geometryViewDir, geometryClearcoatNormal, material, reflectedLight );
	}
	#pragma unroll_loop_end
#endif
#if ( NUM_SPOT_LIGHTS > 0 ) && defined( RE_Direct )
	SpotLight spotLight;
	vec4 spotColor;
	vec3 spotLightCoord;
	bool inSpotLightMap;
	#if defined( USE_SHADOWMAP ) && NUM_SPOT_LIGHT_SHADOWS > 0
	SpotLightShadow spotLightShadow;
	#endif
	#pragma unroll_loop_start
	for ( int i = 0; i < NUM_SPOT_LIGHTS; i ++ ) {
		spotLight = spotLights[ i ];
		getSpotLightInfo( spotLight, geometryPosition, directLight );
		#if ( UNROLLED_LOOP_INDEX < NUM_SPOT_LIGHT_SHADOWS_WITH_MAPS )
		#define SPOT_LIGHT_MAP_INDEX UNROLLED_LOOP_INDEX
		#elif ( UNROLLED_LOOP_INDEX < NUM_SPOT_LIGHT_SHADOWS )
		#define SPOT_LIGHT_MAP_INDEX NUM_SPOT_LIGHT_MAPS
		#else
		#define SPOT_LIGHT_MAP_INDEX ( UNROLLED_LOOP_INDEX - NUM_SPOT_LIGHT_SHADOWS + NUM_SPOT_LIGHT_SHADOWS_WITH_MAPS )
		#endif
		#if ( SPOT_LIGHT_MAP_INDEX < NUM_SPOT_LIGHT_MAPS )
			spotLightCoord = vSpotLightCoord[ i ].xyz / vSpotLightCoord[ i ].w;
			inSpotLightMap = all( lessThan( abs( spotLightCoord * 2. - 1. ), vec3( 1.0 ) ) );
			spotColor = texture2D( spotLightMap[ SPOT_LIGHT_MAP_INDEX ], spotLightCoord.xy );
			directLight.color = inSpotLightMap ? directLight.color * spotColor.rgb : directLight.color;
		#endif
		#undef SPOT_LIGHT_MAP_INDEX
		#if defined( USE_SHADOWMAP ) && ( UNROLLED_LOOP_INDEX < NUM_SPOT_LIGHT_SHADOWS )
		spotLightShadow = spotLightShadows[ i ];
		directLight.color *= ( directLight.visible && receiveShadow ) ? getShadow( spotShadowMap[ i ], spotLightShadow.shadowMapSize, spotLightShadow.shadowIntensity, spotLightShadow.shadowBias, spotLightShadow.shadowRadius, vSpotLightCoord[ i ] ) : 1.0;
		#endif
		RE_Direct( directLight, geometryPosition, geometryNormal, geometryViewDir, geometryClearcoatNormal, material, reflectedLight );
	}
	#pragma unroll_loop_end
#endif
#if ( NUM_SUN_LIGHTS > 0 ) && defined( RE_Direct )
	SunLight sunLight;
	#if defined( USE_SHADOWMAP ) && NUM_SUN_LIGHT_SHADOWS > 0
	SunLightShadow sunLightShadow;
	#endif
	#pragma unroll_loop_start
	for ( int i = 0; i < NUM_SUN_LIGHTS; i ++ ) {
		sunLight = sunLights[ i ];
		getSunLightInfo( sunLight, directLight );
		#if defined( USE_SHADOWMAP ) && ( UNROLLED_LOOP_INDEX < NUM_SUN_LIGHT_SHADOWS )
		sunLightShadow = sunLightShadows[ i ];
		directLight.color *= ( directLight.visible && receiveShadow ) ? getSunShadow( sunShadowMap[ i ], sunLightShadow, UNROLLED_LOOP_INDEX ) : 1.0;
		#endif
		RE_Direct( directLight, geometryPosition, geometryNormal, geometryViewDir, geometryClearcoatNormal, material, reflectedLight );
	}
	#pragma unroll_loop_end
#endif
#if ( NUM_DIR_LIGHTS > 0 ) && defined( RE_Direct )
	DirectionalLight directionalLight;
	#if defined( USE_SHADOWMAP ) && NUM_DIR_LIGHT_SHADOWS > 0
	DirectionalLightShadow directionalLightShadow;
	#endif
	#pragma unroll_loop_start
	for ( int i = 0; i < NUM_DIR_LIGHTS; i ++ ) {
		directionalLight = directionalLights[ i ];
		getDirectionalLightInfo( directionalLight, directLight );
		#if defined( USE_SHADOWMAP ) && ( UNROLLED_LOOP_INDEX < NUM_DIR_LIGHT_SHADOWS )
		directionalLightShadow = directionalLightShadows[ i ];
		directLight.color *= ( directLight.visible && receiveShadow ) ? getShadow( directionalShadowMap[ i ], directionalLightShadow.shadowMapSize, directionalLightShadow.shadowIntensity, directionalLightShadow.shadowBias, directionalLightShadow.shadowRadius, vDirectionalShadowCoord[ i ] ) : 1.0;
		#endif
		RE_Direct( directLight, geometryPosition, geometryNormal, geometryViewDir, geometryClearcoatNormal, material, reflectedLight );
	}
	#pragma unroll_loop_end
#endif
#if ( NUM_RECT_AREA_LIGHTS > 0 ) && defined( RE_Direct_RectArea )
	RectAreaLight rectAreaLight;
	#pragma unroll_loop_start
	for ( int i = 0; i < NUM_RECT_AREA_LIGHTS; i ++ ) {
		rectAreaLight = rectAreaLights[ i ];
		RE_Direct_RectArea( rectAreaLight, geometryPosition, geometryNormal, geometryViewDir, geometryClearcoatNormal, material, reflectedLight );
	}
	#pragma unroll_loop_end
#endif
#if defined( RE_IndirectDiffuse )
	vec3 iblIrradiance = vec3( 0.0 );
	vec3 irradiance = getAmbientLightIrradiance( ambientLightColor );
	#if defined( USE_LIGHT_PROBES )
		irradiance += getLightProbeIrradiance( lightProbe, geometryNormal );
	#endif
	#if ( NUM_HEMI_LIGHTS > 0 )
		#pragma unroll_loop_start
		for ( int i = 0; i < NUM_HEMI_LIGHTS; i ++ ) {
			irradiance += getHemisphereLightIrradiance( hemisphereLights[ i ], geometryNormal );
		}
		#pragma unroll_loop_end
	#endif
	#ifdef USE_LIGHT_PROBES_GRID
		vec3 probeWorldPos = ( ( vec4( geometryPosition, 1.0 ) - viewMatrix[ 3 ] ) * viewMatrix ).xyz;
		vec3 probeWorldNormal = transformNormalByInverseViewMatrix( geometryNormal, viewMatrix );
		irradiance += getLightProbeGridIrradiance( probeWorldPos, probeWorldNormal );
	#endif
#endif
#if defined( RE_IndirectSpecular )
	vec3 radiance = vec3( 0.0 );
	vec3 clearcoatRadiance = vec3( 0.0 );
#endif`,h0=`#if defined( RE_IndirectDiffuse )
	#ifdef USE_LIGHTMAP
		vec4 lightMapTexel = texture2D( lightMap, vLightMapUv );
		vec3 lightMapIrradiance = lightMapTexel.rgb * lightMapIntensity;
		irradiance += lightMapIrradiance;
	#endif
	#if defined( USE_ENVMAP ) && defined( ENVMAP_TYPE_CUBE_UV )
		#if defined( STANDARD ) || defined( LAMBERT ) || defined( PHONG )
			iblIrradiance += getIBLIrradiance( geometryNormal );
		#endif
	#endif
#endif
#if defined( USE_ENVMAP ) && defined( RE_IndirectSpecular )
	#ifdef USE_ANISOTROPY
		vec3 iblRadiance = getIBLAnisotropyRadiance( geometryViewDir, geometryNormal, material.roughness, material.anisotropyB, material.anisotropy );
	#else
		vec3 iblRadiance = getIBLRadiance( geometryViewDir, geometryNormal, material.roughness );
	#endif
	#ifdef USE_RETROREFLECTION
		#ifdef USE_ANISOTROPY
			vec3 retroIBLRadiance = getIBLAnisotropyRetroRadiance( geometryViewDir, geometryNormal, material.roughness, material.anisotropyB, material.anisotropy );
		#else
			vec3 retroIBLRadiance = getIBLRetroRadiance( geometryViewDir, geometryNormal, material.roughness );
		#endif
		iblRadiance = mix( iblRadiance, retroIBLRadiance, saturate( material.retroreflectivity ) );
	#endif
	radiance += iblRadiance;
	#ifdef USE_CLEARCOAT
		clearcoatRadiance += getIBLRadiance( geometryViewDir, geometryClearcoatNormal, material.clearcoatRoughness );
	#endif
#endif`,d0=`#if defined( RE_IndirectDiffuse )
	#if defined( LAMBERT ) || defined( PHONG )
		irradiance += iblIrradiance;
	#endif
	RE_IndirectDiffuse( irradiance, geometryPosition, geometryNormal, geometryViewDir, geometryClearcoatNormal, material, reflectedLight );
#endif
#if defined( RE_IndirectSpecular )
	RE_IndirectSpecular( radiance, iblIrradiance, clearcoatRadiance, geometryPosition, geometryNormal, geometryViewDir, geometryClearcoatNormal, material, reflectedLight );
#endif`,f0=`#ifdef USE_LIGHT_PROBES_GRID
uniform highp sampler3D probesSH;
uniform vec3 probesMin;
uniform vec3 probesMax;
uniform vec3 probesResolution;
vec3 getLightProbeGridIrradiance( vec3 worldPos, vec3 worldNormal ) {
	vec3 res = probesResolution;
	vec3 gridRange = probesMax - probesMin;
	vec3 resMinusOne = res - 1.0;
	vec3 probeSpacing = gridRange / resMinusOne;
	vec3 samplePos = worldPos + worldNormal * probeSpacing * 0.5;
	vec3 uvw = clamp( ( samplePos - probesMin ) / gridRange, 0.0, 1.0 );
	uvw = uvw * resMinusOne / res + 0.5 / res;
	float nz          = res.z;
	float paddedSlices = nz + 2.0;
	float atlasDepth  = 7.0 * paddedSlices;
	float uvZBase     = uvw.z * nz + 1.0;
	vec4 s0 = texture( probesSH, vec3( uvw.xy, ( uvZBase                       ) / atlasDepth ) );
	vec4 s1 = texture( probesSH, vec3( uvw.xy, ( uvZBase +       paddedSlices   ) / atlasDepth ) );
	vec4 s2 = texture( probesSH, vec3( uvw.xy, ( uvZBase + 2.0 * paddedSlices   ) / atlasDepth ) );
	vec4 s3 = texture( probesSH, vec3( uvw.xy, ( uvZBase + 3.0 * paddedSlices   ) / atlasDepth ) );
	vec4 s4 = texture( probesSH, vec3( uvw.xy, ( uvZBase + 4.0 * paddedSlices   ) / atlasDepth ) );
	vec4 s5 = texture( probesSH, vec3( uvw.xy, ( uvZBase + 5.0 * paddedSlices   ) / atlasDepth ) );
	vec4 s6 = texture( probesSH, vec3( uvw.xy, ( uvZBase + 6.0 * paddedSlices   ) / atlasDepth ) );
	vec3 c0 = s0.xyz;
	vec3 c1 = vec3( s0.w, s1.xy );
	vec3 c2 = vec3( s1.zw, s2.x );
	vec3 c3 = s2.yzw;
	vec3 c4 = s3.xyz;
	vec3 c5 = vec3( s3.w, s4.xy );
	vec3 c6 = vec3( s4.zw, s5.x );
	vec3 c7 = s5.yzw;
	vec3 c8 = s6.xyz;
	float x = worldNormal.x, y = worldNormal.y, z = worldNormal.z;
	vec3 result = c0 * 0.886227;
	result += c1 * 2.0 * 0.511664 * y;
	result += c2 * 2.0 * 0.511664 * z;
	result += c3 * 2.0 * 0.511664 * x;
	result += c4 * 2.0 * 0.429043 * x * y;
	result += c5 * 2.0 * 0.429043 * y * z;
	result += c6 * ( 0.743125 * z * z - 0.247708 );
	result += c7 * 2.0 * 0.429043 * x * z;
	result += c8 * 0.429043 * ( x * x - y * y );
	return max( result, vec3( 0.0 ) );
}
#endif`,p0=`#if defined( USE_LOGARITHMIC_DEPTH_BUFFER )
	gl_FragDepth = vIsPerspective == 0.0 ? gl_FragCoord.z : log2( vFragDepth ) * logDepthBufFC * 0.5;
#endif`,m0=`#if defined( USE_LOGARITHMIC_DEPTH_BUFFER )
	uniform float logDepthBufFC;
	varying float vFragDepth;
	varying float vIsPerspective;
#endif`,g0=`#ifdef USE_LOGARITHMIC_DEPTH_BUFFER
	varying float vFragDepth;
	varying float vIsPerspective;
#endif`,x0=`#ifdef USE_LOGARITHMIC_DEPTH_BUFFER
	vFragDepth = 1.0 + gl_Position.w;
	vIsPerspective = float( isPerspectiveMatrix( projectionMatrix ) );
#endif`,v0=`#ifdef USE_MAP
	vec4 sampledDiffuseColor = texture2D( map, vMapUv );
	#ifdef DECODE_VIDEO_TEXTURE
		sampledDiffuseColor = sRGBTransferEOTF( sampledDiffuseColor );
	#endif
	diffuseColor *= sampledDiffuseColor;
#endif`,_0=`#ifdef USE_MAP
	uniform sampler2D map;
#endif`,y0=`#if defined( USE_MAP ) || defined( USE_ALPHAMAP )
	#if defined( USE_POINTS_UV )
		vec2 uv = vUv;
	#else
		vec2 uv = ( uvTransform * vec3( gl_PointCoord.x, 1.0 - gl_PointCoord.y, 1 ) ).xy;
	#endif
#endif
#ifdef USE_MAP
	diffuseColor *= texture2D( map, uv );
#endif
#ifdef USE_ALPHAMAP
	diffuseColor.a *= texture2D( alphaMap, uv ).g;
#endif`,b0=`#if defined( USE_POINTS_UV )
	varying vec2 vUv;
#else
	#if defined( USE_MAP ) || defined( USE_ALPHAMAP )
		uniform mat3 uvTransform;
	#endif
#endif
#ifdef USE_MAP
	uniform sampler2D map;
#endif
#ifdef USE_ALPHAMAP
	uniform sampler2D alphaMap;
#endif`,M0=`float metalnessFactor = metalness;
#ifdef USE_METALNESSMAP
	vec4 texelMetalness = texture2D( metalnessMap, vMetalnessMapUv );
	metalnessFactor *= texelMetalness.b;
#endif`,S0=`#ifdef USE_METALNESSMAP
	uniform sampler2D metalnessMap;
#endif`,w0=`#ifdef USE_INSTANCING_MORPH
	float morphTargetInfluences[ MORPHTARGETS_COUNT ];
	float morphTargetBaseInfluence = texelFetch( morphTexture, ivec2( 0, gl_InstanceID ), 0 ).r;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		morphTargetInfluences[i] =  texelFetch( morphTexture, ivec2( i + 1, gl_InstanceID ), 0 ).r;
	}
#endif`,E0=`#if defined( USE_MORPHCOLORS )
	vColor *= morphTargetBaseInfluence;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		#if defined( USE_COLOR_ALPHA )
			if ( morphTargetInfluences[ i ] != 0.0 ) vColor += getMorph( gl_VertexID, i, 2 ) * morphTargetInfluences[ i ];
		#elif defined( USE_COLOR )
			if ( morphTargetInfluences[ i ] != 0.0 ) vColor += getMorph( gl_VertexID, i, 2 ).rgb * morphTargetInfluences[ i ];
		#endif
	}
#endif`,T0=`#ifdef USE_MORPHNORMALS
	objectNormal *= morphTargetBaseInfluence;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		if ( morphTargetInfluences[ i ] != 0.0 ) objectNormal += getMorph( gl_VertexID, i, 1 ).xyz * morphTargetInfluences[ i ];
	}
#endif`,A0=`#ifdef USE_MORPHTARGETS
	#ifndef USE_INSTANCING_MORPH
		uniform float morphTargetBaseInfluence;
		uniform float morphTargetInfluences[ MORPHTARGETS_COUNT ];
	#endif
	uniform sampler2DArray morphTargetsTexture;
	uniform ivec2 morphTargetsTextureSize;
	vec4 getMorph( const in int vertexIndex, const in int morphTargetIndex, const in int offset ) {
		int texelIndex = vertexIndex * MORPHTARGETS_TEXTURE_STRIDE + offset;
		int y = texelIndex / morphTargetsTextureSize.x;
		int x = texelIndex - y * morphTargetsTextureSize.x;
		ivec3 morphUV = ivec3( x, y, morphTargetIndex );
		return texelFetch( morphTargetsTexture, morphUV, 0 );
	}
#endif`,R0=`#ifdef USE_MORPHTARGETS
	transformed *= morphTargetBaseInfluence;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		if ( morphTargetInfluences[ i ] != 0.0 ) transformed += getMorph( gl_VertexID, i, 0 ).xyz * morphTargetInfluences[ i ];
	}
#endif`,C0=`float faceDirection = gl_FrontFacing ? 1.0 : - 1.0;
#ifdef FLAT_SHADED
	vec3 fdx = dFdx( vViewPosition );
	vec3 fdy = dFdy( vViewPosition );
	vec3 normal = normalize( cross( fdx, fdy ) );
#else
	vec3 normal = normalize( vNormal );
	#ifdef DOUBLE_SIDED
		normal *= faceDirection;
	#endif
#endif
#if defined( USE_NORMALMAP_TANGENTSPACE ) || defined( USE_CLEARCOAT_NORMALMAP ) || defined( USE_ANISOTROPY )
	#ifdef USE_TANGENT
		mat3 tbn = mat3( normalize( vTangent ), normalize( vBitangent ), normal );
	#else
		mat3 tbn = getTangentFrame( - vViewPosition, normal,
		#if defined( USE_NORMALMAP )
			vNormalMapUv
		#elif defined( USE_CLEARCOAT_NORMALMAP )
			vClearcoatNormalMapUv
		#else
			vUv
		#endif
		);
	#endif
	#ifdef DOUBLE_SIDED
		tbn[0] *= faceDirection;
		tbn[1] *= faceDirection;
	#endif
#endif
#ifdef USE_CLEARCOAT_NORMALMAP
	#ifdef USE_TANGENT
		mat3 tbn2 = mat3( normalize( vTangent ), normalize( vBitangent ), normal );
	#else
		mat3 tbn2 = getTangentFrame( - vViewPosition, normal, vClearcoatNormalMapUv );
	#endif
	#ifdef DOUBLE_SIDED
		tbn2[0] *= faceDirection;
		tbn2[1] *= faceDirection;
	#endif
#endif
vec3 nonPerturbedNormal = normal;`,P0=`#ifdef USE_NORMALMAP_OBJECTSPACE
	normal = texture2D( normalMap, vNormalMapUv ).xyz * 2.0 - 1.0;
	#ifdef FLIP_SIDED
		normal = - normal;
	#endif
	#ifdef DOUBLE_SIDED
		normal = normal * faceDirection;
	#endif
	normal = normalize( normalMatrix * normal );
#elif defined( USE_NORMALMAP_TANGENTSPACE )
	vec3 mapN = texture2D( normalMap, vNormalMapUv ).xyz * 2.0 - 1.0;
	#if defined( USE_PACKED_NORMALMAP )
		mapN = vec3( mapN.xy, sqrt( saturate( 1.0 - dot( mapN.xy, mapN.xy ) ) ) );
	#endif
	mapN.xy *= normalScale;
	normal = normalize( tbn * mapN );
#elif defined( USE_BUMPMAP )
	normal = perturbNormalArb( - vViewPosition, normal, dHdxy_fwd(), faceDirection );
#endif`,I0=`#ifndef FLAT_SHADED
	varying vec3 vNormal;
	#ifdef USE_TANGENT
		varying vec3 vTangent;
		varying vec3 vBitangent;
	#endif
#endif`,L0=`#ifndef FLAT_SHADED
	varying vec3 vNormal;
	#ifdef USE_TANGENT
		varying vec3 vTangent;
		varying vec3 vBitangent;
	#endif
#endif`,k0=`#ifndef FLAT_SHADED
	vNormal = normalize( transformedNormal );
	#ifdef USE_TANGENT
		vTangent = normalize( transformedTangent );
		vBitangent = normalize( cross( vNormal, vTangent ) * tangent.w );
		#ifdef FLIP_SIDED
			vBitangent = - vBitangent;
		#endif
	#endif
#endif`,D0=`#ifdef USE_NORMALMAP
	uniform sampler2D normalMap;
	uniform vec2 normalScale;
#endif
#ifdef USE_NORMALMAP_OBJECTSPACE
	uniform mat3 normalMatrix;
#endif
#if ! defined ( USE_TANGENT ) && ( defined ( USE_NORMALMAP_TANGENTSPACE ) || defined ( USE_CLEARCOAT_NORMALMAP ) || defined( USE_ANISOTROPY ) )
	mat3 getTangentFrame( vec3 eye_pos, vec3 surf_norm, vec2 uv ) {
		vec3 q0 = dFdx( eye_pos.xyz );
		vec3 q1 = dFdy( eye_pos.xyz );
		vec2 st0 = dFdx( uv.st );
		vec2 st1 = dFdy( uv.st );
		vec3 N = surf_norm;
		vec3 q1perp = cross( q1, N );
		vec3 q0perp = cross( N, q0 );
		vec3 T = q1perp * st0.x + q0perp * st1.x;
		vec3 B = q1perp * st0.y + q0perp * st1.y;
		float det = max( dot( T, T ), dot( B, B ) );
		float scale = ( det == 0.0 ) ? 0.0 : inversesqrt( det );
		return mat3( T * scale, B * scale, N );
	}
#endif`,N0=`#ifdef USE_CLEARCOAT
	vec3 clearcoatNormal = nonPerturbedNormal;
#endif`,U0=`#ifdef USE_CLEARCOAT_NORMALMAP
	vec3 clearcoatMapN = texture2D( clearcoatNormalMap, vClearcoatNormalMapUv ).xyz * 2.0 - 1.0;
	clearcoatMapN.xy *= clearcoatNormalScale;
	clearcoatNormal = normalize( tbn2 * clearcoatMapN );
#endif`,F0=`#ifdef USE_CLEARCOATMAP
	uniform sampler2D clearcoatMap;
#endif
#ifdef USE_CLEARCOAT_NORMALMAP
	uniform sampler2D clearcoatNormalMap;
	uniform vec2 clearcoatNormalScale;
#endif
#ifdef USE_CLEARCOAT_ROUGHNESSMAP
	uniform sampler2D clearcoatRoughnessMap;
#endif`,O0=`#ifdef USE_IRIDESCENCEMAP
	uniform sampler2D iridescenceMap;
#endif
#ifdef USE_IRIDESCENCE_THICKNESSMAP
	uniform sampler2D iridescenceThicknessMap;
#endif`,B0=`#ifdef OPAQUE
diffuseColor.a = 1.0;
#endif
#ifdef USE_TRANSMISSION
diffuseColor.a *= material.transmissionAlpha;
#endif
gl_FragColor = vec4( outgoingLight, diffuseColor.a );`,z0=`vec3 packNormalToRGB( const in vec3 normal ) {
	return normalize( normal ) * 0.5 + 0.5;
}
vec3 unpackRGBToNormal( const in vec3 rgb ) {
	return 2.0 * rgb.xyz - 1.0;
}
const float PackUpscale = 256. / 255.;const float UnpackDownscale = 255. / 256.;const float ShiftRight8 = 1. / 256.;
const float Inv255 = 1. / 255.;
const vec4 PackFactors = vec4( 1.0, 256.0, 256.0 * 256.0, 256.0 * 256.0 * 256.0 );
const vec2 UnpackFactors2 = vec2( UnpackDownscale, 1.0 / PackFactors.g );
const vec3 UnpackFactors3 = vec3( UnpackDownscale / PackFactors.rg, 1.0 / PackFactors.b );
const vec4 UnpackFactors4 = vec4( UnpackDownscale / PackFactors.rgb, 1.0 / PackFactors.a );
vec4 packDepthToRGBA( const in float v ) {
	if( v <= 0.0 )
		return vec4( 0., 0., 0., 0. );
	if( v >= 1.0 )
		return vec4( 1., 1., 1., 1. );
	float vuf;
	float af = modf( v * PackFactors.a, vuf );
	float bf = modf( vuf * ShiftRight8, vuf );
	float gf = modf( vuf * ShiftRight8, vuf );
	return vec4( vuf * Inv255, gf * PackUpscale, bf * PackUpscale, af );
}
vec3 packDepthToRGB( const in float v ) {
	if( v <= 0.0 )
		return vec3( 0., 0., 0. );
	if( v >= 1.0 )
		return vec3( 1., 1., 1. );
	float vuf;
	float bf = modf( v * PackFactors.b, vuf );
	float gf = modf( vuf * ShiftRight8, vuf );
	return vec3( vuf * Inv255, gf * PackUpscale, bf );
}
vec2 packDepthToRG( const in float v ) {
	if( v <= 0.0 )
		return vec2( 0., 0. );
	if( v >= 1.0 )
		return vec2( 1., 1. );
	float vuf;
	float gf = modf( v * 256., vuf );
	return vec2( vuf * Inv255, gf );
}
float unpackRGBAToDepth( const in vec4 v ) {
	return dot( v, UnpackFactors4 );
}
float unpackRGBToDepth( const in vec3 v ) {
	return dot( v, UnpackFactors3 );
}
float unpackRGToDepth( const in vec2 v ) {
	return v.r * UnpackFactors2.r + v.g * UnpackFactors2.g;
}
vec4 pack2HalfToRGBA( const in vec2 v ) {
	vec4 r = vec4( v.x, fract( v.x * 255.0 ), v.y, fract( v.y * 255.0 ) );
	return vec4( r.x - r.y / 255.0, r.y, r.z - r.w / 255.0, r.w );
}
vec2 unpackRGBATo2Half( const in vec4 v ) {
	return vec2( v.x + ( v.y / 255.0 ), v.z + ( v.w / 255.0 ) );
}
float viewZToOrthographicDepth( const in float viewZ, const in float near, const in float far ) {
	return ( viewZ + near ) / ( near - far );
}
float orthographicDepthToViewZ( const in float depth, const in float near, const in float far ) {
	#ifdef USE_REVERSED_DEPTH_BUFFER
	
		return depth * ( far - near ) - far;
	#else
		return depth * ( near - far ) - near;
	#endif
}
float viewZToPerspectiveDepth( const in float viewZ, const in float near, const in float far ) {
	return ( ( near + viewZ ) * far ) / ( ( far - near ) * viewZ );
}
float perspectiveDepthToViewZ( const in float depth, const in float near, const in float far ) {
	
	#ifdef USE_REVERSED_DEPTH_BUFFER
		return ( near * far ) / ( ( near - far ) * depth - near );
	#else
		return ( near * far ) / ( ( far - near ) * depth - far );
	#endif
}`,H0=`#ifdef PREMULTIPLIED_ALPHA
	gl_FragColor.rgb *= gl_FragColor.a;
#endif`,V0=`vec4 mvPosition = vec4( transformed, 1.0 );
#ifdef USE_BATCHING
	mvPosition = batchingMatrix * mvPosition;
#endif
#ifdef USE_INSTANCING
	mvPosition = instanceMatrix * mvPosition;
#endif
mvPosition = modelViewMatrix * mvPosition;
gl_Position = projectionMatrix * mvPosition;`,G0=`#ifdef DITHERING
	gl_FragColor.rgb = dithering( gl_FragColor.rgb );
#endif`,W0=`#ifdef DITHERING
	vec3 dithering( vec3 color ) {
		float grid_position = rand( gl_FragCoord.xy );
		vec3 dither_shift_RGB = vec3( 0.25 / 255.0, -0.25 / 255.0, 0.25 / 255.0 );
		dither_shift_RGB = mix( 2.0 * dither_shift_RGB, -2.0 * dither_shift_RGB, grid_position );
		return color + dither_shift_RGB;
	}
#endif`,X0=`float roughnessFactor = roughness;
#ifdef USE_ROUGHNESSMAP
	vec4 texelRoughness = texture2D( roughnessMap, vRoughnessMapUv );
	roughnessFactor *= texelRoughness.g;
#endif`,q0=`#ifdef USE_ROUGHNESSMAP
	uniform sampler2D roughnessMap;
#endif`,$0=`#if NUM_SPOT_LIGHT_COORDS > 0
	varying vec4 vSpotLightCoord[ NUM_SPOT_LIGHT_COORDS ];
#endif
#if NUM_SPOT_LIGHT_MAPS > 0
	uniform sampler2D spotLightMap[ NUM_SPOT_LIGHT_MAPS ];
#endif
#ifdef USE_SHADOWMAP
	#if NUM_SUN_LIGHT_SHADOWS > 0
		#define SUN_LIGHT_CASCADES 2
		#if defined( SHADOWMAP_TYPE_PCF )
			uniform sampler2DShadow sunShadowMap[ NUM_SUN_LIGHT_SHADOWS ];
		#else
			uniform sampler2D sunShadowMap[ NUM_SUN_LIGHT_SHADOWS ];
		#endif
		uniform mat4 sunShadowMatrix[ NUM_SUN_LIGHT_SHADOWS * SUN_LIGHT_CASCADES ];
		uniform vec4 sunShadowCascade[ NUM_SUN_LIGHT_SHADOWS * SUN_LIGHT_CASCADES ];
		varying vec4 vSunShadowWorldPosition;
		varying vec3 vSunShadowWorldNormal;
		struct SunLightShadow {
			float shadowIntensity;
			float shadowBias;
			float shadowNormalBias;
			float shadowRadius;
			vec2 shadowMapSize;
		};
		uniform SunLightShadow sunLightShadows[ NUM_SUN_LIGHT_SHADOWS ];
	#endif
	#if NUM_DIR_LIGHT_SHADOWS > 0
		#if defined( SHADOWMAP_TYPE_PCF )
			uniform sampler2DShadow directionalShadowMap[ NUM_DIR_LIGHT_SHADOWS ];
		#else
			uniform sampler2D directionalShadowMap[ NUM_DIR_LIGHT_SHADOWS ];
		#endif
		varying vec4 vDirectionalShadowCoord[ NUM_DIR_LIGHT_SHADOWS ];
		struct DirectionalLightShadow {
			float shadowIntensity;
			float shadowBias;
			float shadowNormalBias;
			float shadowRadius;
			vec2 shadowMapSize;
		};
		uniform DirectionalLightShadow directionalLightShadows[ NUM_DIR_LIGHT_SHADOWS ];
	#endif
	#if NUM_SPOT_LIGHT_SHADOWS > 0
		#if defined( SHADOWMAP_TYPE_PCF )
			uniform sampler2DShadow spotShadowMap[ NUM_SPOT_LIGHT_SHADOWS ];
		#else
			uniform sampler2D spotShadowMap[ NUM_SPOT_LIGHT_SHADOWS ];
		#endif
		struct SpotLightShadow {
			float shadowIntensity;
			float shadowBias;
			float shadowNormalBias;
			float shadowRadius;
			vec2 shadowMapSize;
		};
		uniform SpotLightShadow spotLightShadows[ NUM_SPOT_LIGHT_SHADOWS ];
	#endif
	#if NUM_POINT_LIGHT_SHADOWS > 0
		#if defined( SHADOWMAP_TYPE_PCF )
			uniform samplerCubeShadow pointShadowMap[ NUM_POINT_LIGHT_SHADOWS ];
		#elif defined( SHADOWMAP_TYPE_BASIC )
			uniform samplerCube pointShadowMap[ NUM_POINT_LIGHT_SHADOWS ];
		#endif
		varying vec4 vPointShadowCoord[ NUM_POINT_LIGHT_SHADOWS ];
		struct PointLightShadow {
			float shadowIntensity;
			float shadowBias;
			float shadowNormalBias;
			float shadowRadius;
			vec2 shadowMapSize;
			float shadowCameraNear;
			float shadowCameraFar;
		};
		uniform PointLightShadow pointLightShadows[ NUM_POINT_LIGHT_SHADOWS ];
	#endif
	#if defined( SHADOWMAP_TYPE_PCF )
		float interleavedGradientNoise( vec2 position ) {
			return fract( 52.9829189 * fract( dot( position, vec2( 0.06711056, 0.00583715 ) ) ) );
		}
		vec2 vogelDiskSample( int sampleIndex, int samplesCount, float phi ) {
			const float goldenAngle = 2.399963229728653;
			float r = sqrt( ( float( sampleIndex ) + 0.5 ) / float( samplesCount ) );
			float theta = float( sampleIndex ) * goldenAngle + phi;
			return vec2( cos( theta ), sin( theta ) ) * r;
		}
	#endif
	#if defined( SHADOWMAP_TYPE_PCF )
		float getShadow( sampler2DShadow shadowMap, vec2 shadowMapSize, float shadowIntensity, float shadowBias, float shadowRadius, vec4 shadowCoord ) {
			float shadow = 1.0;
			shadowCoord.xyz /= shadowCoord.w;
			shadowCoord.z += shadowBias;
			bool inFrustum = shadowCoord.x >= 0.0 && shadowCoord.x <= 1.0 && shadowCoord.y >= 0.0 && shadowCoord.y <= 1.0;
			bool frustumTest = inFrustum && shadowCoord.z <= 1.0;
			if ( frustumTest ) {
				vec2 texelSize = vec2( 1.0 ) / shadowMapSize;
				float radius = shadowRadius * texelSize.x;
				float phi = interleavedGradientNoise( gl_FragCoord.xy ) * PI2;
				shadow = (
					texture( shadowMap, vec3( shadowCoord.xy + vogelDiskSample( 0, 5, phi ) * radius, shadowCoord.z ) ) +
					texture( shadowMap, vec3( shadowCoord.xy + vogelDiskSample( 1, 5, phi ) * radius, shadowCoord.z ) ) +
					texture( shadowMap, vec3( shadowCoord.xy + vogelDiskSample( 2, 5, phi ) * radius, shadowCoord.z ) ) +
					texture( shadowMap, vec3( shadowCoord.xy + vogelDiskSample( 3, 5, phi ) * radius, shadowCoord.z ) ) +
					texture( shadowMap, vec3( shadowCoord.xy + vogelDiskSample( 4, 5, phi ) * radius, shadowCoord.z ) )
				) * 0.2;
			}
			return mix( 1.0, shadow, shadowIntensity );
		}
	#elif defined( SHADOWMAP_TYPE_VSM )
		float getShadow( sampler2D shadowMap, vec2 shadowMapSize, float shadowIntensity, float shadowBias, float shadowRadius, vec4 shadowCoord ) {
			float shadow = 1.0;
			shadowCoord.xyz /= shadowCoord.w;
			#ifdef USE_REVERSED_DEPTH_BUFFER
				shadowCoord.z -= shadowBias;
			#else
				shadowCoord.z += shadowBias;
			#endif
			bool inFrustum = shadowCoord.x >= 0.0 && shadowCoord.x <= 1.0 && shadowCoord.y >= 0.0 && shadowCoord.y <= 1.0;
			bool frustumTest = inFrustum && shadowCoord.z <= 1.0;
			if ( frustumTest ) {
				vec2 distribution = texture2D( shadowMap, shadowCoord.xy ).rg;
				float mean = distribution.x;
				float variance = distribution.y * distribution.y;
				#ifdef USE_REVERSED_DEPTH_BUFFER
					float hard_shadow = step( mean, shadowCoord.z );
				#else
					float hard_shadow = step( shadowCoord.z, mean );
				#endif
				
				if ( hard_shadow == 1.0 ) {
					shadow = 1.0;
				} else {
					variance = max( variance, 0.0000001 );
					float d = shadowCoord.z - mean;
					float p_max = variance / ( variance + d * d );
					p_max = clamp( ( p_max - 0.3 ) / 0.65, 0.0, 1.0 );
					shadow = max( hard_shadow, p_max );
				}
			}
			return mix( 1.0, shadow, shadowIntensity );
		}
	#else
		float getShadow( sampler2D shadowMap, vec2 shadowMapSize, float shadowIntensity, float shadowBias, float shadowRadius, vec4 shadowCoord ) {
			float shadow = 1.0;
			shadowCoord.xyz /= shadowCoord.w;
			#ifdef USE_REVERSED_DEPTH_BUFFER
				shadowCoord.z -= shadowBias;
			#else
				shadowCoord.z += shadowBias;
			#endif
			bool inFrustum = shadowCoord.x >= 0.0 && shadowCoord.x <= 1.0 && shadowCoord.y >= 0.0 && shadowCoord.y <= 1.0;
			bool frustumTest = inFrustum && shadowCoord.z <= 1.0;
			if ( frustumTest ) {
				float depth = texture2D( shadowMap, shadowCoord.xy ).r;
				#ifdef USE_REVERSED_DEPTH_BUFFER
					shadow = step( depth, shadowCoord.z );
				#else
					shadow = step( shadowCoord.z, depth );
				#endif
			}
			return mix( 1.0, shadow, shadowIntensity );
		}
	#endif
	#if NUM_SUN_LIGHT_SHADOWS > 0
		float getSunShadow(
			#if defined( SHADOWMAP_TYPE_PCF )
				sampler2DShadow shadowMap,
			#else
				sampler2D shadowMap,
			#endif
			SunLightShadow sunLightShadow,
			int shadowIndex
		) {
			vec4 shadowWorldPosition = vec4( vSunShadowWorldPosition.xyz + vSunShadowWorldNormal * sunLightShadow.shadowNormalBias, 1.0 );
			float viewDepth = vSunShadowWorldPosition.w;
			int cascadeOffset = shadowIndex * SUN_LIGHT_CASCADES;
			float shadow = 1.0;
			for ( int i = SUN_LIGHT_CASCADES - 1; i >= 0; i -- ) {
				vec4 cascade = sunShadowCascade[ cascadeOffset + i ];
				if ( viewDepth >= cascade.x && viewDepth < cascade.y ) {
					float cascadeShadow = getShadow(
						shadowMap,
						sunLightShadow.shadowMapSize,
						sunLightShadow.shadowIntensity,
						sunLightShadow.shadowBias,
						sunLightShadow.shadowRadius,
						sunShadowMatrix[ cascadeOffset + i ] * shadowWorldPosition
					);
					shadow = mix( cascadeShadow, shadow, smoothstep( cascade.z, cascade.y, viewDepth ) );
				}
			}
			return shadow;
		}
	#endif
	#if NUM_POINT_LIGHT_SHADOWS > 0
	#if defined( SHADOWMAP_TYPE_PCF )
	float getPointShadow( samplerCubeShadow shadowMap, vec2 shadowMapSize, float shadowIntensity, float shadowBias, float shadowRadius, vec4 shadowCoord, float shadowCameraNear, float shadowCameraFar ) {
		float shadow = 1.0;
		vec3 lightToPosition = shadowCoord.xyz;
		vec3 bd3D = normalize( lightToPosition );
		vec3 absVec = abs( lightToPosition );
		float viewSpaceZ = max( max( absVec.x, absVec.y ), absVec.z );
		if ( viewSpaceZ - shadowCameraFar <= 0.0 && viewSpaceZ - shadowCameraNear >= 0.0 ) {
			#ifdef USE_REVERSED_DEPTH_BUFFER
				float dp = ( shadowCameraNear * ( shadowCameraFar - viewSpaceZ ) ) / ( viewSpaceZ * ( shadowCameraFar - shadowCameraNear ) );
				dp -= shadowBias;
			#else
				float dp = ( shadowCameraFar * ( viewSpaceZ - shadowCameraNear ) ) / ( viewSpaceZ * ( shadowCameraFar - shadowCameraNear ) );
				dp += shadowBias;
			#endif
			float texelSize = shadowRadius / shadowMapSize.x;
			vec3 absDir = abs( bd3D );
			vec3 tangent = absDir.x > absDir.z ? vec3( 0.0, 1.0, 0.0 ) : vec3( 1.0, 0.0, 0.0 );
			tangent = normalize( cross( bd3D, tangent ) );
			vec3 bitangent = cross( bd3D, tangent );
			float phi = interleavedGradientNoise( gl_FragCoord.xy ) * PI2;
			vec2 sample0 = vogelDiskSample( 0, 5, phi );
			vec2 sample1 = vogelDiskSample( 1, 5, phi );
			vec2 sample2 = vogelDiskSample( 2, 5, phi );
			vec2 sample3 = vogelDiskSample( 3, 5, phi );
			vec2 sample4 = vogelDiskSample( 4, 5, phi );
			shadow = (
				texture( shadowMap, vec4( bd3D + ( tangent * sample0.x + bitangent * sample0.y ) * texelSize, dp ) ) +
				texture( shadowMap, vec4( bd3D + ( tangent * sample1.x + bitangent * sample1.y ) * texelSize, dp ) ) +
				texture( shadowMap, vec4( bd3D + ( tangent * sample2.x + bitangent * sample2.y ) * texelSize, dp ) ) +
				texture( shadowMap, vec4( bd3D + ( tangent * sample3.x + bitangent * sample3.y ) * texelSize, dp ) ) +
				texture( shadowMap, vec4( bd3D + ( tangent * sample4.x + bitangent * sample4.y ) * texelSize, dp ) )
			) * 0.2;
		}
		return mix( 1.0, shadow, shadowIntensity );
	}
	#elif defined( SHADOWMAP_TYPE_BASIC )
	float getPointShadow( samplerCube shadowMap, vec2 shadowMapSize, float shadowIntensity, float shadowBias, float shadowRadius, vec4 shadowCoord, float shadowCameraNear, float shadowCameraFar ) {
		float shadow = 1.0;
		vec3 lightToPosition = shadowCoord.xyz;
		vec3 absVec = abs( lightToPosition );
		float viewSpaceZ = max( max( absVec.x, absVec.y ), absVec.z );
		if ( viewSpaceZ - shadowCameraFar <= 0.0 && viewSpaceZ - shadowCameraNear >= 0.0 ) {
			float dp = ( shadowCameraFar * ( viewSpaceZ - shadowCameraNear ) ) / ( viewSpaceZ * ( shadowCameraFar - shadowCameraNear ) );
			dp += shadowBias;
			vec3 bd3D = normalize( lightToPosition );
			float depth = textureCube( shadowMap, bd3D ).r;
			#ifdef USE_REVERSED_DEPTH_BUFFER
				depth = 1.0 - depth;
			#endif
			shadow = step( dp, depth );
		}
		return mix( 1.0, shadow, shadowIntensity );
	}
	#endif
	#endif
#endif`,Y0=`#if NUM_SPOT_LIGHT_COORDS > 0
	uniform mat4 spotLightMatrix[ NUM_SPOT_LIGHT_COORDS ];
	varying vec4 vSpotLightCoord[ NUM_SPOT_LIGHT_COORDS ];
#endif
#ifdef USE_SHADOWMAP
	#if NUM_SUN_LIGHT_SHADOWS > 0
		varying vec4 vSunShadowWorldPosition;
		varying vec3 vSunShadowWorldNormal;
	#endif
	#if NUM_DIR_LIGHT_SHADOWS > 0
		uniform mat4 directionalShadowMatrix[ NUM_DIR_LIGHT_SHADOWS ];
		varying vec4 vDirectionalShadowCoord[ NUM_DIR_LIGHT_SHADOWS ];
		struct DirectionalLightShadow {
			float shadowIntensity;
			float shadowBias;
			float shadowNormalBias;
			float shadowRadius;
			vec2 shadowMapSize;
		};
		uniform DirectionalLightShadow directionalLightShadows[ NUM_DIR_LIGHT_SHADOWS ];
	#endif
	#if NUM_SPOT_LIGHT_SHADOWS > 0
		struct SpotLightShadow {
			float shadowIntensity;
			float shadowBias;
			float shadowNormalBias;
			float shadowRadius;
			vec2 shadowMapSize;
		};
		uniform SpotLightShadow spotLightShadows[ NUM_SPOT_LIGHT_SHADOWS ];
	#endif
	#if NUM_POINT_LIGHT_SHADOWS > 0
		uniform mat4 pointShadowMatrix[ NUM_POINT_LIGHT_SHADOWS ];
		varying vec4 vPointShadowCoord[ NUM_POINT_LIGHT_SHADOWS ];
		struct PointLightShadow {
			float shadowIntensity;
			float shadowBias;
			float shadowNormalBias;
			float shadowRadius;
			vec2 shadowMapSize;
			float shadowCameraNear;
			float shadowCameraFar;
		};
		uniform PointLightShadow pointLightShadows[ NUM_POINT_LIGHT_SHADOWS ];
	#endif
#endif`,Z0=`#if ( defined( USE_SHADOWMAP ) && ( NUM_DIR_LIGHT_SHADOWS > 0 || NUM_SUN_LIGHT_SHADOWS > 0 || NUM_POINT_LIGHT_SHADOWS > 0 ) ) || ( NUM_SPOT_LIGHT_COORDS > 0 )
	#ifdef HAS_NORMAL
		vec3 shadowWorldNormal = transformNormalByInverseViewMatrix( transformedNormal, viewMatrix );
	#else
		vec3 shadowWorldNormal = vec3( 0.0 );
	#endif
	vec4 shadowWorldPosition;
#endif
#if defined( USE_SHADOWMAP )
	#if NUM_SUN_LIGHT_SHADOWS > 0
		vSunShadowWorldPosition = vec4( worldPosition.xyz, - mvPosition.z );
		vSunShadowWorldNormal = shadowWorldNormal;
	#endif
	#if NUM_DIR_LIGHT_SHADOWS > 0
		#pragma unroll_loop_start
		for ( int i = 0; i < NUM_DIR_LIGHT_SHADOWS; i ++ ) {
			shadowWorldPosition = worldPosition + vec4( shadowWorldNormal * directionalLightShadows[ i ].shadowNormalBias, 0 );
			vDirectionalShadowCoord[ i ] = directionalShadowMatrix[ i ] * shadowWorldPosition;
		}
		#pragma unroll_loop_end
	#endif
	#if NUM_POINT_LIGHT_SHADOWS > 0
		#pragma unroll_loop_start
		for ( int i = 0; i < NUM_POINT_LIGHT_SHADOWS; i ++ ) {
			shadowWorldPosition = worldPosition + vec4( shadowWorldNormal * pointLightShadows[ i ].shadowNormalBias, 0 );
			vPointShadowCoord[ i ] = pointShadowMatrix[ i ] * shadowWorldPosition;
		}
		#pragma unroll_loop_end
	#endif
#endif
#if NUM_SPOT_LIGHT_COORDS > 0
	#pragma unroll_loop_start
	for ( int i = 0; i < NUM_SPOT_LIGHT_COORDS; i ++ ) {
		shadowWorldPosition = worldPosition;
		#if ( defined( USE_SHADOWMAP ) && UNROLLED_LOOP_INDEX < NUM_SPOT_LIGHT_SHADOWS )
			shadowWorldPosition.xyz += shadowWorldNormal * spotLightShadows[ i ].shadowNormalBias;
		#endif
		vSpotLightCoord[ i ] = spotLightMatrix[ i ] * shadowWorldPosition;
	}
	#pragma unroll_loop_end
#endif`,K0=`float getShadowMask() {
	float shadow = 1.0;
	#ifdef USE_SHADOWMAP
	#if NUM_SUN_LIGHT_SHADOWS > 0
	SunLightShadow sunLight;
	#pragma unroll_loop_start
	for ( int i = 0; i < NUM_SUN_LIGHT_SHADOWS; i ++ ) {
		sunLight = sunLightShadows[ i ];
		shadow *= receiveShadow ? getSunShadow( sunShadowMap[ i ], sunLight, UNROLLED_LOOP_INDEX ) : 1.0;
	}
	#pragma unroll_loop_end
	#endif
	#if NUM_DIR_LIGHT_SHADOWS > 0
	DirectionalLightShadow directionalLight;
	#pragma unroll_loop_start
	for ( int i = 0; i < NUM_DIR_LIGHT_SHADOWS; i ++ ) {
		directionalLight = directionalLightShadows[ i ];
		shadow *= receiveShadow ? getShadow( directionalShadowMap[ i ], directionalLight.shadowMapSize, directionalLight.shadowIntensity, directionalLight.shadowBias, directionalLight.shadowRadius, vDirectionalShadowCoord[ i ] ) : 1.0;
	}
	#pragma unroll_loop_end
	#endif
	#if NUM_SPOT_LIGHT_SHADOWS > 0
	SpotLightShadow spotLight;
	#pragma unroll_loop_start
	for ( int i = 0; i < NUM_SPOT_LIGHT_SHADOWS; i ++ ) {
		spotLight = spotLightShadows[ i ];
		shadow *= receiveShadow ? getShadow( spotShadowMap[ i ], spotLight.shadowMapSize, spotLight.shadowIntensity, spotLight.shadowBias, spotLight.shadowRadius, vSpotLightCoord[ i ] ) : 1.0;
	}
	#pragma unroll_loop_end
	#endif
	#if NUM_POINT_LIGHT_SHADOWS > 0 && ( defined( SHADOWMAP_TYPE_PCF ) || defined( SHADOWMAP_TYPE_BASIC ) )
	PointLightShadow pointLight;
	#pragma unroll_loop_start
	for ( int i = 0; i < NUM_POINT_LIGHT_SHADOWS; i ++ ) {
		pointLight = pointLightShadows[ i ];
		shadow *= receiveShadow ? getPointShadow( pointShadowMap[ i ], pointLight.shadowMapSize, pointLight.shadowIntensity, pointLight.shadowBias, pointLight.shadowRadius, vPointShadowCoord[ i ], pointLight.shadowCameraNear, pointLight.shadowCameraFar ) : 1.0;
	}
	#pragma unroll_loop_end
	#endif
	#endif
	return shadow;
}`,J0=`#ifdef USE_SKINNING
	mat4 boneMatX = getBoneMatrix( skinIndex.x );
	mat4 boneMatY = getBoneMatrix( skinIndex.y );
	mat4 boneMatZ = getBoneMatrix( skinIndex.z );
	mat4 boneMatW = getBoneMatrix( skinIndex.w );
#endif`,j0=`#ifdef USE_SKINNING
	uniform mat4 bindMatrix;
	uniform mat4 bindMatrixInverse;
	uniform highp sampler2D boneTexture;
	mat4 getBoneMatrix( const in float i ) {
		int size = textureSize( boneTexture, 0 ).x;
		int j = int( i ) * 4;
		int x = j % size;
		int y = j / size;
		vec4 v1 = texelFetch( boneTexture, ivec2( x, y ), 0 );
		vec4 v2 = texelFetch( boneTexture, ivec2( x + 1, y ), 0 );
		vec4 v3 = texelFetch( boneTexture, ivec2( x + 2, y ), 0 );
		vec4 v4 = texelFetch( boneTexture, ivec2( x + 3, y ), 0 );
		return mat4( v1, v2, v3, v4 );
	}
#endif`,Q0=`#ifdef USE_SKINNING
	vec4 skinVertex = bindMatrix * vec4( transformed, 1.0 );
	vec4 skinned = vec4( 0.0 );
	skinned += boneMatX * skinVertex * skinWeight.x;
	skinned += boneMatY * skinVertex * skinWeight.y;
	skinned += boneMatZ * skinVertex * skinWeight.z;
	skinned += boneMatW * skinVertex * skinWeight.w;
	transformed = ( bindMatrixInverse * skinned ).xyz;
#endif`,eg=`#ifdef USE_SKINNING
	mat4 skinMatrix = mat4( 0.0 );
	skinMatrix += skinWeight.x * boneMatX;
	skinMatrix += skinWeight.y * boneMatY;
	skinMatrix += skinWeight.z * boneMatZ;
	skinMatrix += skinWeight.w * boneMatW;
	skinMatrix = bindMatrixInverse * skinMatrix * bindMatrix;
	objectNormal = vec4( skinMatrix * vec4( objectNormal, 0.0 ) ).xyz;
	#ifdef USE_TANGENT
		objectTangent = vec4( skinMatrix * vec4( objectTangent, 0.0 ) ).xyz;
	#endif
#endif`,tg=`float specularStrength;
#ifdef USE_SPECULARMAP
	vec4 texelSpecular = texture2D( specularMap, vSpecularMapUv );
	specularStrength = texelSpecular.r;
#else
	specularStrength = 1.0;
#endif`,ng=`#ifdef USE_SPECULARMAP
	uniform sampler2D specularMap;
#endif`,ig=`#if defined( TONE_MAPPING )
	gl_FragColor.rgb = toneMapping( gl_FragColor.rgb );
#endif`,rg=`#ifndef saturate
#define saturate( a ) clamp( a, 0.0, 1.0 )
#endif
uniform float toneMappingExposure;
vec3 LinearToneMapping( vec3 color ) {
	return saturate( toneMappingExposure * color );
}
vec3 ReinhardToneMapping( vec3 color ) {
	color *= toneMappingExposure;
	return saturate( color / ( vec3( 1.0 ) + color ) );
}
vec3 CineonToneMapping( vec3 color ) {
	color *= toneMappingExposure;
	color = max( vec3( 0.0 ), color - 0.004 );
	return pow( ( color * ( 6.2 * color + 0.5 ) ) / ( color * ( 6.2 * color + 1.7 ) + 0.06 ), vec3( 2.2 ) );
}
vec3 RRTAndODTFit( vec3 v ) {
	vec3 a = v * ( v + 0.0245786 ) - 0.000090537;
	vec3 b = v * ( 0.983729 * v + 0.4329510 ) + 0.238081;
	return a / b;
}
vec3 ACESFilmicToneMapping( vec3 color ) {
	const mat3 ACESInputMat = mat3(
		vec3( 0.59719, 0.07600, 0.02840 ),		vec3( 0.35458, 0.90834, 0.13383 ),
		vec3( 0.04823, 0.01566, 0.83777 )
	);
	const mat3 ACESOutputMat = mat3(
		vec3(  1.60475, -0.10208, -0.00327 ),		vec3( -0.53108,  1.10813, -0.07276 ),
		vec3( -0.07367, -0.00605,  1.07602 )
	);
	color *= toneMappingExposure / 0.6;
	color = ACESInputMat * color;
	color = RRTAndODTFit( color );
	color = ACESOutputMat * color;
	return saturate( color );
}
const mat3 LINEAR_REC2020_TO_LINEAR_SRGB = mat3(
	vec3( 1.6605, - 0.1246, - 0.0182 ),
	vec3( - 0.5876, 1.1329, - 0.1006 ),
	vec3( - 0.0728, - 0.0083, 1.1187 )
);
const mat3 LINEAR_SRGB_TO_LINEAR_REC2020 = mat3(
	vec3( 0.6274, 0.0691, 0.0164 ),
	vec3( 0.3293, 0.9195, 0.0880 ),
	vec3( 0.0433, 0.0113, 0.8956 )
);
vec3 agxDefaultContrastApprox( vec3 x ) {
	vec3 x2 = x * x;
	vec3 x4 = x2 * x2;
	return + 15.5 * x4 * x2
		- 40.14 * x4 * x
		+ 31.96 * x4
		- 6.868 * x2 * x
		+ 0.4298 * x2
		+ 0.1191 * x
		- 0.00232;
}
vec3 AgXToneMapping( vec3 color ) {
	const mat3 AgXInsetMatrix = mat3(
		vec3( 0.856627153315983, 0.137318972929847, 0.11189821299995 ),
		vec3( 0.0951212405381588, 0.761241990602591, 0.0767994186031903 ),
		vec3( 0.0482516061458583, 0.101439036467562, 0.811302368396859 )
	);
	const mat3 AgXOutsetMatrix = mat3(
		vec3( 1.1271005818144368, - 0.1413297634984383, - 0.14132976349843826 ),
		vec3( - 0.11060664309660323, 1.157823702216272, - 0.11060664309660294 ),
		vec3( - 0.016493938717834573, - 0.016493938717834257, 1.2519364065950405 )
	);
	const float AgxMinEv = - 12.47393;	const float AgxMaxEv = 4.026069;
	color *= toneMappingExposure;
	color = LINEAR_SRGB_TO_LINEAR_REC2020 * color;
	color = AgXInsetMatrix * color;
	color = max( color, 1e-10 );	color = log2( color );
	color = ( color - AgxMinEv ) / ( AgxMaxEv - AgxMinEv );
	color = clamp( color, 0.0, 1.0 );
	color = agxDefaultContrastApprox( color );
	color = AgXOutsetMatrix * color;
	color = pow( max( vec3( 0.0 ), color ), vec3( 2.2 ) );
	color = LINEAR_REC2020_TO_LINEAR_SRGB * color;
	color = clamp( color, 0.0, 1.0 );
	return color;
}
vec3 NeutralToneMapping( vec3 color ) {
	const float StartCompression = 0.8 - 0.04;
	const float Desaturation = 0.15;
	color *= toneMappingExposure;
	float x = min( color.r, min( color.g, color.b ) );
	float offset = x < 0.08 ? x - 6.25 * x * x : 0.04;
	color -= offset;
	float peak = max( color.r, max( color.g, color.b ) );
	if ( peak < StartCompression ) return color;
	float d = 1. - StartCompression;
	float newPeak = 1. - d * d / ( peak + d - StartCompression );
	color *= newPeak / peak;
	float g = 1. - 1. / ( Desaturation * ( peak - newPeak ) + 1. );
	return mix( color, vec3( newPeak ), g );
}
vec3 CustomToneMapping( vec3 color ) { return color; }`,sg=`#ifdef USE_TRANSMISSION
	material.transmission = transmission;
	material.transmissionAlpha = 1.0;
	material.thickness = thickness;
	material.attenuationDistance = attenuationDistance;
	material.attenuationColor = attenuationColor;
	#ifdef USE_TRANSMISSIONMAP
		material.transmission *= texture2D( transmissionMap, vTransmissionMapUv ).r;
	#endif
	#ifdef USE_THICKNESSMAP
		material.thickness *= texture2D( thicknessMap, vThicknessMapUv ).g;
	#endif
	vec3 pos = vWorldPosition;
	vec3 v = normalize( cameraPosition - pos );
	vec3 n = transformNormalByInverseViewMatrix( normal, viewMatrix );
	vec4 transmitted = getIBLVolumeRefraction(
		n, v, material.roughness, material.diffuseContribution, material.specularColorBlended, material.specularF90,
		pos, modelMatrix, viewMatrix, projectionMatrix, material.dispersion, material.ior, material.thickness,
		material.attenuationColor, material.attenuationDistance );
	material.transmissionAlpha = mix( material.transmissionAlpha, transmitted.a, material.transmission );
	totalDiffuse = mix( totalDiffuse, transmitted.rgb, material.transmission );
#endif`,og=`#ifdef USE_TRANSMISSION
	uniform float transmission;
	uniform float thickness;
	uniform float attenuationDistance;
	uniform vec3 attenuationColor;
	#ifdef USE_TRANSMISSIONMAP
		uniform sampler2D transmissionMap;
	#endif
	#ifdef USE_THICKNESSMAP
		uniform sampler2D thicknessMap;
	#endif
	uniform vec2 transmissionSamplerSize;
	uniform sampler2D transmissionSamplerMap;
	uniform mat4 modelMatrix;
	uniform mat4 projectionMatrix;
	varying vec3 vWorldPosition;
	float w0( float a ) {
		return ( 1.0 / 6.0 ) * ( a * ( a * ( - a + 3.0 ) - 3.0 ) + 1.0 );
	}
	float w1( float a ) {
		return ( 1.0 / 6.0 ) * ( a *  a * ( 3.0 * a - 6.0 ) + 4.0 );
	}
	float w2( float a ){
		return ( 1.0 / 6.0 ) * ( a * ( a * ( - 3.0 * a + 3.0 ) + 3.0 ) + 1.0 );
	}
	float w3( float a ) {
		return ( 1.0 / 6.0 ) * ( a * a * a );
	}
	float g0( float a ) {
		return w0( a ) + w1( a );
	}
	float g1( float a ) {
		return w2( a ) + w3( a );
	}
	float h0( float a ) {
		return - 1.0 + w1( a ) / ( w0( a ) + w1( a ) );
	}
	float h1( float a ) {
		return 1.0 + w3( a ) / ( w2( a ) + w3( a ) );
	}
	vec4 bicubic( sampler2D tex, vec2 uv, vec4 texelSize, float lod ) {
		uv = uv * texelSize.zw + 0.5;
		vec2 iuv = floor( uv );
		vec2 fuv = fract( uv );
		float g0x = g0( fuv.x );
		float g1x = g1( fuv.x );
		float h0x = h0( fuv.x );
		float h1x = h1( fuv.x );
		float h0y = h0( fuv.y );
		float h1y = h1( fuv.y );
		vec2 p0 = ( vec2( iuv.x + h0x, iuv.y + h0y ) - 0.5 ) * texelSize.xy;
		vec2 p1 = ( vec2( iuv.x + h1x, iuv.y + h0y ) - 0.5 ) * texelSize.xy;
		vec2 p2 = ( vec2( iuv.x + h0x, iuv.y + h1y ) - 0.5 ) * texelSize.xy;
		vec2 p3 = ( vec2( iuv.x + h1x, iuv.y + h1y ) - 0.5 ) * texelSize.xy;
		return g0( fuv.y ) * ( g0x * textureLod( tex, p0, lod ) + g1x * textureLod( tex, p1, lod ) ) +
			g1( fuv.y ) * ( g0x * textureLod( tex, p2, lod ) + g1x * textureLod( tex, p3, lod ) );
	}
	vec4 textureBicubic( sampler2D sampler, vec2 uv, float lod ) {
		vec2 fLodSize = vec2( textureSize( sampler, int( lod ) ) );
		vec2 cLodSize = vec2( textureSize( sampler, int( lod + 1.0 ) ) );
		vec2 fLodSizeInv = 1.0 / fLodSize;
		vec2 cLodSizeInv = 1.0 / cLodSize;
		vec4 fSample = bicubic( sampler, uv, vec4( fLodSizeInv, fLodSize ), floor( lod ) );
		vec4 cSample = bicubic( sampler, uv, vec4( cLodSizeInv, cLodSize ), ceil( lod ) );
		return mix( fSample, cSample, fract( lod ) );
	}
	vec3 getVolumeTransmissionRay( const in vec3 n, const in vec3 v, const in float thickness, const in float ior, const in mat4 modelMatrix ) {
		vec3 refractionVector = refract( - v, normalize( n ), 1.0 / ior );
		vec3 modelScale;
		modelScale.x = length( vec3( modelMatrix[ 0 ].xyz ) );
		modelScale.y = length( vec3( modelMatrix[ 1 ].xyz ) );
		modelScale.z = length( vec3( modelMatrix[ 2 ].xyz ) );
		return normalize( refractionVector ) * thickness * modelScale;
	}
	float applyIorToRoughness( const in float roughness, const in float ior ) {
		return roughness * clamp( ior * 2.0 - 2.0, 0.0, 1.0 );
	}
	vec4 getTransmissionSample( const in vec2 fragCoord, const in float roughness, const in float ior ) {
		float lod = log2( transmissionSamplerSize.x ) * applyIorToRoughness( roughness, ior );
		return textureBicubic( transmissionSamplerMap, fragCoord.xy, lod );
	}
	vec3 volumeAttenuation( const in float transmissionDistance, const in vec3 attenuationColor, const in float attenuationDistance ) {
		if ( isinf( attenuationDistance ) ) {
			return vec3( 1.0 );
		} else {
			vec3 attenuationCoefficient = -log( attenuationColor ) / attenuationDistance;
			vec3 transmittance = exp( - attenuationCoefficient * transmissionDistance );			return transmittance;
		}
	}
	vec4 getIBLVolumeRefraction( const in vec3 n, const in vec3 v, const in float roughness, const in vec3 diffuseColor,
		const in vec3 specularColor, const in float specularF90, const in vec3 position, const in mat4 modelMatrix,
		const in mat4 viewMatrix, const in mat4 projMatrix, const in float dispersion, const in float ior, const in float thickness,
		const in vec3 attenuationColor, const in float attenuationDistance ) {
		vec4 transmittedLight;
		vec3 transmittance;
		#ifdef USE_DISPERSION
			float halfSpread = ( ior - 1.0 ) * 0.025 * dispersion;
			vec3 iors = vec3( ior - halfSpread, ior, ior + halfSpread );
			for ( int i = 0; i < 3; i ++ ) {
				vec3 transmissionRay = getVolumeTransmissionRay( n, v, thickness, iors[ i ], modelMatrix );
				vec3 refractedRayExit = position + transmissionRay;
				vec4 ndcPos = projMatrix * viewMatrix * vec4( refractedRayExit, 1.0 );
				vec2 refractionCoords = ndcPos.xy / ndcPos.w;
				refractionCoords += 1.0;
				refractionCoords /= 2.0;
				vec4 transmissionSample = getTransmissionSample( refractionCoords, roughness, iors[ i ] );
				transmittedLight[ i ] = transmissionSample[ i ];
				transmittedLight.a += transmissionSample.a;
				transmittance[ i ] = diffuseColor[ i ] * volumeAttenuation( length( transmissionRay ), attenuationColor, attenuationDistance )[ i ];
			}
			transmittedLight.a /= 3.0;
		#else
			vec3 transmissionRay = getVolumeTransmissionRay( n, v, thickness, ior, modelMatrix );
			vec3 refractedRayExit = position + transmissionRay;
			vec4 ndcPos = projMatrix * viewMatrix * vec4( refractedRayExit, 1.0 );
			vec2 refractionCoords = ndcPos.xy / ndcPos.w;
			refractionCoords += 1.0;
			refractionCoords /= 2.0;
			transmittedLight = getTransmissionSample( refractionCoords, roughness, ior );
			transmittance = diffuseColor * volumeAttenuation( length( transmissionRay ), attenuationColor, attenuationDistance );
		#endif
		vec3 attenuatedColor = transmittance * transmittedLight.rgb;
		vec3 F = EnvironmentBRDF( n, v, specularColor, specularF90, roughness );
		float transmittanceFactor = ( transmittance.r + transmittance.g + transmittance.b ) / 3.0;
		return vec4( ( 1.0 - F ) * attenuatedColor, 1.0 - ( 1.0 - transmittedLight.a ) * transmittanceFactor );
	}
#endif`,ag=`#if defined( USE_UV ) || defined( USE_ANISOTROPY )
	varying vec2 vUv;
#endif
#ifdef USE_MAP
	varying vec2 vMapUv;
#endif
#ifdef USE_ALPHAMAP
	varying vec2 vAlphaMapUv;
#endif
#ifdef USE_LIGHTMAP
	varying vec2 vLightMapUv;
#endif
#ifdef USE_AOMAP
	varying vec2 vAoMapUv;
#endif
#ifdef USE_BUMPMAP
	varying vec2 vBumpMapUv;
#endif
#ifdef USE_NORMALMAP
	varying vec2 vNormalMapUv;
#endif
#ifdef USE_EMISSIVEMAP
	varying vec2 vEmissiveMapUv;
#endif
#ifdef USE_METALNESSMAP
	varying vec2 vMetalnessMapUv;
#endif
#ifdef USE_ROUGHNESSMAP
	varying vec2 vRoughnessMapUv;
#endif
#ifdef USE_ANISOTROPYMAP
	varying vec2 vAnisotropyMapUv;
#endif
#ifdef USE_CLEARCOATMAP
	varying vec2 vClearcoatMapUv;
#endif
#ifdef USE_CLEARCOAT_NORMALMAP
	varying vec2 vClearcoatNormalMapUv;
#endif
#ifdef USE_CLEARCOAT_ROUGHNESSMAP
	varying vec2 vClearcoatRoughnessMapUv;
#endif
#ifdef USE_IRIDESCENCEMAP
	varying vec2 vIridescenceMapUv;
#endif
#ifdef USE_IRIDESCENCE_THICKNESSMAP
	varying vec2 vIridescenceThicknessMapUv;
#endif
#ifdef USE_SHEEN_COLORMAP
	varying vec2 vSheenColorMapUv;
#endif
#ifdef USE_SHEEN_ROUGHNESSMAP
	varying vec2 vSheenRoughnessMapUv;
#endif
#ifdef USE_SPECULARMAP
	varying vec2 vSpecularMapUv;
#endif
#ifdef USE_SPECULAR_COLORMAP
	varying vec2 vSpecularColorMapUv;
#endif
#ifdef USE_SPECULAR_INTENSITYMAP
	varying vec2 vSpecularIntensityMapUv;
#endif
#ifdef USE_TRANSMISSIONMAP
	uniform mat3 transmissionMapTransform;
	varying vec2 vTransmissionMapUv;
#endif
#ifdef USE_THICKNESSMAP
	uniform mat3 thicknessMapTransform;
	varying vec2 vThicknessMapUv;
#endif`,lg=`#if defined( USE_UV ) || defined( USE_ANISOTROPY )
	varying vec2 vUv;
#endif
#ifdef USE_MAP
	uniform mat3 mapTransform;
	varying vec2 vMapUv;
#endif
#ifdef USE_ALPHAMAP
	uniform mat3 alphaMapTransform;
	varying vec2 vAlphaMapUv;
#endif
#ifdef USE_LIGHTMAP
	uniform mat3 lightMapTransform;
	varying vec2 vLightMapUv;
#endif
#ifdef USE_AOMAP
	uniform mat3 aoMapTransform;
	varying vec2 vAoMapUv;
#endif
#ifdef USE_BUMPMAP
	uniform mat3 bumpMapTransform;
	varying vec2 vBumpMapUv;
#endif
#ifdef USE_NORMALMAP
	uniform mat3 normalMapTransform;
	varying vec2 vNormalMapUv;
#endif
#ifdef USE_DISPLACEMENTMAP
	uniform mat3 displacementMapTransform;
	varying vec2 vDisplacementMapUv;
#endif
#ifdef USE_EMISSIVEMAP
	uniform mat3 emissiveMapTransform;
	varying vec2 vEmissiveMapUv;
#endif
#ifdef USE_METALNESSMAP
	uniform mat3 metalnessMapTransform;
	varying vec2 vMetalnessMapUv;
#endif
#ifdef USE_ROUGHNESSMAP
	uniform mat3 roughnessMapTransform;
	varying vec2 vRoughnessMapUv;
#endif
#ifdef USE_ANISOTROPYMAP
	uniform mat3 anisotropyMapTransform;
	varying vec2 vAnisotropyMapUv;
#endif
#ifdef USE_CLEARCOATMAP
	uniform mat3 clearcoatMapTransform;
	varying vec2 vClearcoatMapUv;
#endif
#ifdef USE_CLEARCOAT_NORMALMAP
	uniform mat3 clearcoatNormalMapTransform;
	varying vec2 vClearcoatNormalMapUv;
#endif
#ifdef USE_CLEARCOAT_ROUGHNESSMAP
	uniform mat3 clearcoatRoughnessMapTransform;
	varying vec2 vClearcoatRoughnessMapUv;
#endif
#ifdef USE_SHEEN_COLORMAP
	uniform mat3 sheenColorMapTransform;
	varying vec2 vSheenColorMapUv;
#endif
#ifdef USE_SHEEN_ROUGHNESSMAP
	uniform mat3 sheenRoughnessMapTransform;
	varying vec2 vSheenRoughnessMapUv;
#endif
#ifdef USE_IRIDESCENCEMAP
	uniform mat3 iridescenceMapTransform;
	varying vec2 vIridescenceMapUv;
#endif
#ifdef USE_IRIDESCENCE_THICKNESSMAP
	uniform mat3 iridescenceThicknessMapTransform;
	varying vec2 vIridescenceThicknessMapUv;
#endif
#ifdef USE_SPECULARMAP
	uniform mat3 specularMapTransform;
	varying vec2 vSpecularMapUv;
#endif
#ifdef USE_SPECULAR_COLORMAP
	uniform mat3 specularColorMapTransform;
	varying vec2 vSpecularColorMapUv;
#endif
#ifdef USE_SPECULAR_INTENSITYMAP
	uniform mat3 specularIntensityMapTransform;
	varying vec2 vSpecularIntensityMapUv;
#endif
#ifdef USE_TRANSMISSIONMAP
	uniform mat3 transmissionMapTransform;
	varying vec2 vTransmissionMapUv;
#endif
#ifdef USE_THICKNESSMAP
	uniform mat3 thicknessMapTransform;
	varying vec2 vThicknessMapUv;
#endif`,cg=`#if defined( USE_UV ) || defined( USE_ANISOTROPY )
	vUv = vec3( uv, 1 ).xy;
#endif
#ifdef USE_MAP
	vMapUv = ( mapTransform * vec3( MAP_UV, 1 ) ).xy;
#endif
#ifdef USE_ALPHAMAP
	vAlphaMapUv = ( alphaMapTransform * vec3( ALPHAMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_LIGHTMAP
	vLightMapUv = ( lightMapTransform * vec3( LIGHTMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_AOMAP
	vAoMapUv = ( aoMapTransform * vec3( AOMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_BUMPMAP
	vBumpMapUv = ( bumpMapTransform * vec3( BUMPMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_NORMALMAP
	vNormalMapUv = ( normalMapTransform * vec3( NORMALMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_DISPLACEMENTMAP
	vDisplacementMapUv = ( displacementMapTransform * vec3( DISPLACEMENTMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_EMISSIVEMAP
	vEmissiveMapUv = ( emissiveMapTransform * vec3( EMISSIVEMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_METALNESSMAP
	vMetalnessMapUv = ( metalnessMapTransform * vec3( METALNESSMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_ROUGHNESSMAP
	vRoughnessMapUv = ( roughnessMapTransform * vec3( ROUGHNESSMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_ANISOTROPYMAP
	vAnisotropyMapUv = ( anisotropyMapTransform * vec3( ANISOTROPYMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_CLEARCOATMAP
	vClearcoatMapUv = ( clearcoatMapTransform * vec3( CLEARCOATMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_CLEARCOAT_NORMALMAP
	vClearcoatNormalMapUv = ( clearcoatNormalMapTransform * vec3( CLEARCOAT_NORMALMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_CLEARCOAT_ROUGHNESSMAP
	vClearcoatRoughnessMapUv = ( clearcoatRoughnessMapTransform * vec3( CLEARCOAT_ROUGHNESSMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_IRIDESCENCEMAP
	vIridescenceMapUv = ( iridescenceMapTransform * vec3( IRIDESCENCEMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_IRIDESCENCE_THICKNESSMAP
	vIridescenceThicknessMapUv = ( iridescenceThicknessMapTransform * vec3( IRIDESCENCE_THICKNESSMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_SHEEN_COLORMAP
	vSheenColorMapUv = ( sheenColorMapTransform * vec3( SHEEN_COLORMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_SHEEN_ROUGHNESSMAP
	vSheenRoughnessMapUv = ( sheenRoughnessMapTransform * vec3( SHEEN_ROUGHNESSMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_SPECULARMAP
	vSpecularMapUv = ( specularMapTransform * vec3( SPECULARMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_SPECULAR_COLORMAP
	vSpecularColorMapUv = ( specularColorMapTransform * vec3( SPECULAR_COLORMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_SPECULAR_INTENSITYMAP
	vSpecularIntensityMapUv = ( specularIntensityMapTransform * vec3( SPECULAR_INTENSITYMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_TRANSMISSIONMAP
	vTransmissionMapUv = ( transmissionMapTransform * vec3( TRANSMISSIONMAP_UV, 1 ) ).xy;
#endif
#ifdef USE_THICKNESSMAP
	vThicknessMapUv = ( thicknessMapTransform * vec3( THICKNESSMAP_UV, 1 ) ).xy;
#endif`,ug=`#if defined( USE_ENVMAP ) || defined( DISTANCE ) || defined ( USE_SHADOWMAP ) || defined ( USE_TRANSMISSION ) || NUM_SPOT_LIGHT_COORDS > 0
	vec4 worldPosition = vec4( transformed, 1.0 );
	#ifdef USE_BATCHING
		worldPosition = batchingMatrix * worldPosition;
	#endif
	#ifdef USE_INSTANCING
		worldPosition = instanceMatrix * worldPosition;
	#endif
	worldPosition = modelMatrix * worldPosition;
#endif`,hg=`varying vec2 vUv;
uniform mat3 uvTransform;
void main() {
	vUv = ( uvTransform * vec3( uv, 1 ) ).xy;
	gl_Position = vec4( position.xy, 1.0, 1.0 );
}`,dg=`uniform sampler2D t2D;
uniform float backgroundIntensity;
varying vec2 vUv;
void main() {
	vec4 texColor = texture2D( t2D, vUv );
	#ifdef DECODE_VIDEO_TEXTURE
		texColor = vec4( mix( pow( texColor.rgb * 0.9478672986 + vec3( 0.0521327014 ), vec3( 2.4 ) ), texColor.rgb * 0.0773993808, vec3( lessThanEqual( texColor.rgb, vec3( 0.04045 ) ) ) ), texColor.w );
	#endif
	texColor.rgb *= backgroundIntensity;
	gl_FragColor = texColor;
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
}`,fg=`varying vec3 vWorldDirection;
#include <common>
void main() {
	vWorldDirection = transformDirection( position, modelMatrix );
	#include <begin_vertex>
	#include <project_vertex>
	gl_Position.z = gl_Position.w;
}`,pg=`#ifdef ENVMAP_TYPE_CUBE
	uniform samplerCube envMap;
#elif defined( ENVMAP_TYPE_CUBE_UV )
	uniform sampler2D envMap;
#endif
uniform float backgroundBlurriness;
uniform float backgroundIntensity;
uniform mat3 backgroundRotation;
varying vec3 vWorldDirection;
#include <cube_uv_reflection_fragment>
void main() {
	#ifdef ENVMAP_TYPE_CUBE
		vec4 texColor = textureCube( envMap, backgroundRotation * vWorldDirection );
	#elif defined( ENVMAP_TYPE_CUBE_UV )
		vec4 texColor = textureCubeUV( envMap, backgroundRotation * vWorldDirection, backgroundBlurriness );
	#else
		vec4 texColor = vec4( 0.0, 0.0, 0.0, 1.0 );
	#endif
	texColor.rgb *= backgroundIntensity;
	gl_FragColor = texColor;
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
}`,mg=`varying vec3 vWorldDirection;
#include <common>
void main() {
	vWorldDirection = transformDirection( position, modelMatrix );
	#include <begin_vertex>
	#include <project_vertex>
	gl_Position.z = gl_Position.w;
}`,gg=`uniform samplerCube tCube;
uniform float tFlip;
uniform float opacity;
varying vec3 vWorldDirection;
void main() {
	vec4 texColor = textureCube( tCube, vec3( tFlip * vWorldDirection.x, vWorldDirection.yz ) );
	gl_FragColor = texColor;
	gl_FragColor.a *= opacity;
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
}`,xg=`#include <common>
#include <batching_pars_vertex>
#include <uv_pars_vertex>
#include <displacementmap_pars_vertex>
#include <morphtarget_pars_vertex>
#include <skinning_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>
varying vec2 vHighPrecisionZW;
void main() {
	#include <uv_vertex>
	#include <batching_vertex>
	#include <skinbase_vertex>
	#include <morphinstance_vertex>
	#ifdef USE_DISPLACEMENTMAP
		#include <beginnormal_vertex>
		#include <morphnormal_vertex>
		#include <skinnormal_vertex>
	#endif
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <skinning_vertex>
	#include <displacementmap_vertex>
	#include <project_vertex>
	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
	vHighPrecisionZW = gl_Position.zw;
}`,vg=`#if DEPTH_PACKING == 3200
	uniform float opacity;
#endif
#include <common>
#include <packing>
#include <uv_pars_fragment>
#include <map_pars_fragment>
#include <alphamap_pars_fragment>
#include <alphatest_pars_fragment>
#include <alphahash_pars_fragment>
#include <logdepthbuf_pars_fragment>
#include <clipping_planes_pars_fragment>
varying vec2 vHighPrecisionZW;
void main() {
	vec4 diffuseColor = vec4( 1.0 );
	#include <clipping_planes_fragment>
	#if DEPTH_PACKING == 3200
		diffuseColor.a = opacity;
	#endif
	#include <map_fragment>
	#include <alphamap_fragment>
	#include <alphatest_fragment>
	#include <alphahash_fragment>
	#include <logdepthbuf_fragment>
	#ifdef USE_REVERSED_DEPTH_BUFFER
		float fragCoordZ = vHighPrecisionZW[ 0 ] / vHighPrecisionZW[ 1 ];
	#else
		float fragCoordZ = 0.5 * vHighPrecisionZW[ 0 ] / vHighPrecisionZW[ 1 ] + 0.5;
	#endif
	#if DEPTH_PACKING == 3200
		gl_FragColor = vec4( vec3( 1.0 - fragCoordZ ), opacity );
	#elif DEPTH_PACKING == 3201
		gl_FragColor = packDepthToRGBA( fragCoordZ );
	#elif DEPTH_PACKING == 3202
		gl_FragColor = vec4( packDepthToRGB( fragCoordZ ), 1.0 );
	#elif DEPTH_PACKING == 3203
		gl_FragColor = vec4( packDepthToRG( fragCoordZ ), 0.0, 1.0 );
	#endif
}`,_g=`#define DISTANCE
varying vec3 vWorldPosition;
#include <common>
#include <batching_pars_vertex>
#include <uv_pars_vertex>
#include <displacementmap_pars_vertex>
#include <morphtarget_pars_vertex>
#include <skinning_pars_vertex>
#include <clipping_planes_pars_vertex>
void main() {
	#include <uv_vertex>
	#include <batching_vertex>
	#include <skinbase_vertex>
	#include <morphinstance_vertex>
	#ifdef USE_DISPLACEMENTMAP
		#include <beginnormal_vertex>
		#include <morphnormal_vertex>
		#include <skinnormal_vertex>
	#endif
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <skinning_vertex>
	#include <displacementmap_vertex>
	#include <project_vertex>
	#include <worldpos_vertex>
	#include <clipping_planes_vertex>
	vWorldPosition = worldPosition.xyz;
}`,yg=`#define DISTANCE
uniform vec3 referencePosition;
uniform float nearDistance;
uniform float farDistance;
varying vec3 vWorldPosition;
#include <common>
#include <uv_pars_fragment>
#include <map_pars_fragment>
#include <alphamap_pars_fragment>
#include <alphatest_pars_fragment>
#include <alphahash_pars_fragment>
#include <clipping_planes_pars_fragment>
void main() {
	vec4 diffuseColor = vec4( 1.0 );
	#include <clipping_planes_fragment>
	#include <map_fragment>
	#include <alphamap_fragment>
	#include <alphatest_fragment>
	#include <alphahash_fragment>
	float dist = length( vWorldPosition - referencePosition );
	dist = ( dist - nearDistance ) / ( farDistance - nearDistance );
	dist = saturate( dist );
	gl_FragColor = vec4( dist, 0.0, 0.0, 1.0 );
}`,bg=`varying vec3 vWorldDirection;
#include <common>
void main() {
	vWorldDirection = transformDirection( position, modelMatrix );
	#include <begin_vertex>
	#include <project_vertex>
}`,Mg=`uniform sampler2D tEquirect;
varying vec3 vWorldDirection;
#include <common>
void main() {
	vec3 direction = normalize( vWorldDirection );
	vec2 sampleUV = equirectUv( direction );
	gl_FragColor = texture2D( tEquirect, sampleUV );
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
}`,Sg=`uniform float scale;
attribute float lineDistance;
varying float vLineDistance;
#include <common>
#include <uv_pars_vertex>
#include <color_pars_vertex>
#include <fog_pars_vertex>
#include <morphtarget_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>
void main() {
	vLineDistance = scale * lineDistance;
	#include <uv_vertex>
	#include <color_vertex>
	#include <morphinstance_vertex>
	#include <morphcolor_vertex>
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <project_vertex>
	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
	#include <fog_vertex>
}`,wg=`uniform vec3 diffuse;
uniform float opacity;
uniform float dashSize;
uniform float totalSize;
varying float vLineDistance;
#include <common>
#include <color_pars_fragment>
#include <uv_pars_fragment>
#include <map_pars_fragment>
#include <fog_pars_fragment>
#include <logdepthbuf_pars_fragment>
#include <clipping_planes_pars_fragment>
void main() {
	vec4 diffuseColor = vec4( diffuse, opacity );
	#include <clipping_planes_fragment>
	if ( mod( vLineDistance, totalSize ) > dashSize ) {
		discard;
	}
	vec3 outgoingLight = vec3( 0.0 );
	#include <logdepthbuf_fragment>
	#include <map_fragment>
	#include <color_fragment>
	outgoingLight = diffuseColor.rgb;
	#include <opaque_fragment>
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
	#include <fog_fragment>
	#include <premultiplied_alpha_fragment>
}`,Eg=`#include <common>
#include <batching_pars_vertex>
#include <uv_pars_vertex>
#include <envmap_pars_vertex>
#include <color_pars_vertex>
#include <fog_pars_vertex>
#include <morphtarget_pars_vertex>
#include <skinning_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>
void main() {
	#include <uv_vertex>
	#include <color_vertex>
	#include <morphinstance_vertex>
	#include <morphcolor_vertex>
	#include <batching_vertex>
	#if defined ( USE_ENVMAP ) || defined ( USE_SKINNING )
		#include <beginnormal_vertex>
		#include <morphnormal_vertex>
		#include <skinbase_vertex>
		#include <skinnormal_vertex>
		#include <defaultnormal_vertex>
	#endif
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <skinning_vertex>
	#include <project_vertex>
	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
	#include <worldpos_vertex>
	#include <envmap_vertex>
	#include <fog_vertex>
}`,Tg=`uniform vec3 diffuse;
uniform float opacity;
#ifndef FLAT_SHADED
	varying vec3 vNormal;
#endif
#include <common>
#include <dithering_pars_fragment>
#include <color_pars_fragment>
#include <uv_pars_fragment>
#include <map_pars_fragment>
#include <alphamap_pars_fragment>
#include <alphatest_pars_fragment>
#include <alphahash_pars_fragment>
#include <aomap_pars_fragment>
#include <lightmap_pars_fragment>
#include <envmap_common_pars_fragment>
#include <envmap_pars_fragment>
#include <fog_pars_fragment>
#include <specularmap_pars_fragment>
#include <logdepthbuf_pars_fragment>
#include <clipping_planes_pars_fragment>
void main() {
	vec4 diffuseColor = vec4( diffuse, opacity );
	#include <clipping_planes_fragment>
	#include <logdepthbuf_fragment>
	#include <map_fragment>
	#include <color_fragment>
	#include <alphamap_fragment>
	#include <alphatest_fragment>
	#include <alphahash_fragment>
	#include <specularmap_fragment>
	ReflectedLight reflectedLight = ReflectedLight( vec3( 0.0 ), vec3( 0.0 ), vec3( 0.0 ), vec3( 0.0 ) );
	#ifdef USE_LIGHTMAP
		vec4 lightMapTexel = texture2D( lightMap, vLightMapUv );
		reflectedLight.indirectDiffuse += lightMapTexel.rgb * lightMapIntensity * RECIPROCAL_PI;
	#else
		reflectedLight.indirectDiffuse += vec3( 1.0 );
	#endif
	#include <aomap_fragment>
	reflectedLight.indirectDiffuse *= diffuseColor.rgb;
	vec3 outgoingLight = reflectedLight.indirectDiffuse;
	#include <envmap_fragment>
	#include <opaque_fragment>
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
	#include <fog_fragment>
	#include <premultiplied_alpha_fragment>
	#include <dithering_fragment>
}`,Ag=`#define LAMBERT
varying vec3 vViewPosition;
#include <common>
#include <batching_pars_vertex>
#include <uv_pars_vertex>
#include <displacementmap_pars_vertex>
#include <envmap_pars_vertex>
#include <color_pars_vertex>
#include <fog_pars_vertex>
#include <normal_pars_vertex>
#include <morphtarget_pars_vertex>
#include <skinning_pars_vertex>
#include <shadowmap_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>
void main() {
	#include <uv_vertex>
	#include <color_vertex>
	#include <morphinstance_vertex>
	#include <morphcolor_vertex>
	#include <batching_vertex>
	#include <beginnormal_vertex>
	#include <morphnormal_vertex>
	#include <skinbase_vertex>
	#include <skinnormal_vertex>
	#include <defaultnormal_vertex>
	#include <normal_vertex>
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <skinning_vertex>
	#include <displacementmap_vertex>
	#include <project_vertex>
	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
	vViewPosition = - mvPosition.xyz;
	#include <worldpos_vertex>
	#include <envmap_vertex>
	#include <shadowmap_vertex>
	#include <fog_vertex>
}`,Rg=`#define LAMBERT
uniform vec3 diffuse;
uniform vec3 emissive;
uniform float opacity;
#include <common>
#include <dithering_pars_fragment>
#include <color_pars_fragment>
#include <uv_pars_fragment>
#include <map_pars_fragment>
#include <alphamap_pars_fragment>
#include <alphatest_pars_fragment>
#include <alphahash_pars_fragment>
#include <aomap_pars_fragment>
#include <lightmap_pars_fragment>
#include <emissivemap_pars_fragment>
#include <cube_uv_reflection_fragment>
#include <envmap_common_pars_fragment>
#include <envmap_pars_fragment>
#include <envmap_physical_pars_fragment>
#include <fog_pars_fragment>
#include <bsdfs>
#include <lights_pars_begin>
#include <normal_pars_fragment>
#include <lights_lambert_pars_fragment>
#include <shadowmap_pars_fragment>
#include <bumpmap_pars_fragment>
#include <normalmap_pars_fragment>
#include <specularmap_pars_fragment>
#include <logdepthbuf_pars_fragment>
#include <clipping_planes_pars_fragment>
void main() {
	vec4 diffuseColor = vec4( diffuse, opacity );
	#include <clipping_planes_fragment>
	ReflectedLight reflectedLight = ReflectedLight( vec3( 0.0 ), vec3( 0.0 ), vec3( 0.0 ), vec3( 0.0 ) );
	vec3 totalEmissiveRadiance = emissive;
	#include <logdepthbuf_fragment>
	#include <map_fragment>
	#include <color_fragment>
	#include <alphamap_fragment>
	#include <alphatest_fragment>
	#include <alphahash_fragment>
	#include <specularmap_fragment>
	#include <normal_fragment_begin>
	#include <normal_fragment_maps>
	#include <emissivemap_fragment>
	#include <lights_lambert_fragment>
	#include <lights_fragment_begin>
	#include <lights_fragment_maps>
	#include <lights_fragment_end>
	#include <aomap_fragment>
	vec3 outgoingLight = reflectedLight.directDiffuse + reflectedLight.indirectDiffuse + totalEmissiveRadiance;
	#include <envmap_fragment>
	#include <opaque_fragment>
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
	#include <fog_fragment>
	#include <premultiplied_alpha_fragment>
	#include <dithering_fragment>
}`,Cg=`#define MATCAP
varying vec3 vViewPosition;
#include <common>
#include <batching_pars_vertex>
#include <uv_pars_vertex>
#include <color_pars_vertex>
#include <displacementmap_pars_vertex>
#include <fog_pars_vertex>
#include <normal_pars_vertex>
#include <morphtarget_pars_vertex>
#include <skinning_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>
void main() {
	#include <uv_vertex>
	#include <color_vertex>
	#include <morphinstance_vertex>
	#include <morphcolor_vertex>
	#include <batching_vertex>
	#include <beginnormal_vertex>
	#include <morphnormal_vertex>
	#include <skinbase_vertex>
	#include <skinnormal_vertex>
	#include <defaultnormal_vertex>
	#include <normal_vertex>
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <skinning_vertex>
	#include <displacementmap_vertex>
	#include <project_vertex>
	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
	#include <fog_vertex>
	vViewPosition = - mvPosition.xyz;
}`,Pg=`#define MATCAP
uniform vec3 diffuse;
uniform float opacity;
uniform sampler2D matcap;
varying vec3 vViewPosition;
#include <common>
#include <dithering_pars_fragment>
#include <color_pars_fragment>
#include <uv_pars_fragment>
#include <map_pars_fragment>
#include <alphamap_pars_fragment>
#include <alphatest_pars_fragment>
#include <alphahash_pars_fragment>
#include <fog_pars_fragment>
#include <normal_pars_fragment>
#include <bumpmap_pars_fragment>
#include <normalmap_pars_fragment>
#include <logdepthbuf_pars_fragment>
#include <clipping_planes_pars_fragment>
void main() {
	vec4 diffuseColor = vec4( diffuse, opacity );
	#include <clipping_planes_fragment>
	#include <logdepthbuf_fragment>
	#include <map_fragment>
	#include <color_fragment>
	#include <alphamap_fragment>
	#include <alphatest_fragment>
	#include <alphahash_fragment>
	#include <normal_fragment_begin>
	#include <normal_fragment_maps>
	vec3 viewDir = normalize( vViewPosition );
	vec3 x = normalize( vec3( viewDir.z, 0.0, - viewDir.x ) );
	vec3 y = cross( viewDir, x );
	vec2 uv = vec2( dot( x, normal ), dot( y, normal ) ) * 0.495 + 0.5;
	#ifdef USE_MATCAP
		vec4 matcapColor = texture2D( matcap, uv );
	#else
		vec4 matcapColor = vec4( vec3( mix( 0.2, 0.8, uv.y ) ), 1.0 );
	#endif
	vec3 outgoingLight = diffuseColor.rgb * matcapColor.rgb;
	#include <opaque_fragment>
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
	#include <fog_fragment>
	#include <premultiplied_alpha_fragment>
	#include <dithering_fragment>
}`,Ig=`#define NORMAL
#if defined( FLAT_SHADED ) || defined( USE_BUMPMAP ) || defined( USE_NORMALMAP_TANGENTSPACE )
	varying vec3 vViewPosition;
#endif
#include <common>
#include <batching_pars_vertex>
#include <uv_pars_vertex>
#include <displacementmap_pars_vertex>
#include <normal_pars_vertex>
#include <morphtarget_pars_vertex>
#include <skinning_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>
void main() {
	#include <uv_vertex>
	#include <batching_vertex>
	#include <beginnormal_vertex>
	#include <morphinstance_vertex>
	#include <morphnormal_vertex>
	#include <skinbase_vertex>
	#include <skinnormal_vertex>
	#include <defaultnormal_vertex>
	#include <normal_vertex>
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <skinning_vertex>
	#include <displacementmap_vertex>
	#include <project_vertex>
	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
#if defined( FLAT_SHADED ) || defined( USE_BUMPMAP ) || defined( USE_NORMALMAP_TANGENTSPACE )
	vViewPosition = - mvPosition.xyz;
#endif
}`,Lg=`#define NORMAL
uniform float opacity;
#if defined( FLAT_SHADED ) || defined( USE_BUMPMAP ) || defined( USE_NORMALMAP_TANGENTSPACE )
	varying vec3 vViewPosition;
#endif
#include <uv_pars_fragment>
#include <normal_pars_fragment>
#include <bumpmap_pars_fragment>
#include <normalmap_pars_fragment>
#include <logdepthbuf_pars_fragment>
#include <clipping_planes_pars_fragment>
void main() {
	vec4 diffuseColor = vec4( 0.0, 0.0, 0.0, opacity );
	#include <clipping_planes_fragment>
	#include <logdepthbuf_fragment>
	#include <normal_fragment_begin>
	#include <normal_fragment_maps>
	gl_FragColor = vec4( normalize( normal ) * 0.5 + 0.5, diffuseColor.a );
	#ifdef OPAQUE
		gl_FragColor.a = 1.0;
	#endif
}`,kg=`#define PHONG
varying vec3 vViewPosition;
#include <common>
#include <batching_pars_vertex>
#include <uv_pars_vertex>
#include <displacementmap_pars_vertex>
#include <envmap_pars_vertex>
#include <color_pars_vertex>
#include <fog_pars_vertex>
#include <normal_pars_vertex>
#include <morphtarget_pars_vertex>
#include <skinning_pars_vertex>
#include <shadowmap_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>
void main() {
	#include <uv_vertex>
	#include <color_vertex>
	#include <morphcolor_vertex>
	#include <batching_vertex>
	#include <beginnormal_vertex>
	#include <morphinstance_vertex>
	#include <morphnormal_vertex>
	#include <skinbase_vertex>
	#include <skinnormal_vertex>
	#include <defaultnormal_vertex>
	#include <normal_vertex>
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <skinning_vertex>
	#include <displacementmap_vertex>
	#include <project_vertex>
	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
	vViewPosition = - mvPosition.xyz;
	#include <worldpos_vertex>
	#include <envmap_vertex>
	#include <shadowmap_vertex>
	#include <fog_vertex>
}`,Dg=`#define PHONG
uniform vec3 diffuse;
uniform vec3 emissive;
uniform vec3 specular;
uniform float shininess;
uniform float opacity;
#include <common>
#include <dithering_pars_fragment>
#include <color_pars_fragment>
#include <uv_pars_fragment>
#include <map_pars_fragment>
#include <alphamap_pars_fragment>
#include <alphatest_pars_fragment>
#include <alphahash_pars_fragment>
#include <aomap_pars_fragment>
#include <lightmap_pars_fragment>
#include <emissivemap_pars_fragment>
#include <cube_uv_reflection_fragment>
#include <envmap_common_pars_fragment>
#include <envmap_pars_fragment>
#include <envmap_physical_pars_fragment>
#include <fog_pars_fragment>
#include <bsdfs>
#include <lights_pars_begin>
#include <normal_pars_fragment>
#include <lights_phong_pars_fragment>
#include <shadowmap_pars_fragment>
#include <bumpmap_pars_fragment>
#include <normalmap_pars_fragment>
#include <specularmap_pars_fragment>
#include <logdepthbuf_pars_fragment>
#include <clipping_planes_pars_fragment>
void main() {
	vec4 diffuseColor = vec4( diffuse, opacity );
	#include <clipping_planes_fragment>
	ReflectedLight reflectedLight = ReflectedLight( vec3( 0.0 ), vec3( 0.0 ), vec3( 0.0 ), vec3( 0.0 ) );
	vec3 totalEmissiveRadiance = emissive;
	#include <logdepthbuf_fragment>
	#include <map_fragment>
	#include <color_fragment>
	#include <alphamap_fragment>
	#include <alphatest_fragment>
	#include <alphahash_fragment>
	#include <specularmap_fragment>
	#include <normal_fragment_begin>
	#include <normal_fragment_maps>
	#include <emissivemap_fragment>
	#include <lights_phong_fragment>
	#include <lights_fragment_begin>
	#include <lights_fragment_maps>
	#include <lights_fragment_end>
	#include <aomap_fragment>
	vec3 outgoingLight = reflectedLight.directDiffuse + reflectedLight.indirectDiffuse + reflectedLight.directSpecular + reflectedLight.indirectSpecular + totalEmissiveRadiance;
	#include <envmap_fragment>
	#include <opaque_fragment>
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
	#include <fog_fragment>
	#include <premultiplied_alpha_fragment>
	#include <dithering_fragment>
}`,Ng=`#define STANDARD
varying vec3 vViewPosition;
#ifdef USE_TRANSMISSION
	varying vec3 vWorldPosition;
#endif
#include <common>
#include <batching_pars_vertex>
#include <uv_pars_vertex>
#include <displacementmap_pars_vertex>
#include <color_pars_vertex>
#include <fog_pars_vertex>
#include <normal_pars_vertex>
#include <morphtarget_pars_vertex>
#include <skinning_pars_vertex>
#include <shadowmap_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>
void main() {
	#include <uv_vertex>
	#include <color_vertex>
	#include <morphinstance_vertex>
	#include <morphcolor_vertex>
	#include <batching_vertex>
	#include <beginnormal_vertex>
	#include <morphnormal_vertex>
	#include <skinbase_vertex>
	#include <skinnormal_vertex>
	#include <defaultnormal_vertex>
	#include <normal_vertex>
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <skinning_vertex>
	#include <displacementmap_vertex>
	#include <project_vertex>
	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
	vViewPosition = - mvPosition.xyz;
	#include <worldpos_vertex>
	#include <shadowmap_vertex>
	#include <fog_vertex>
#ifdef USE_TRANSMISSION
	vWorldPosition = worldPosition.xyz;
#endif
}`,Ug=`#define STANDARD
#ifdef PHYSICAL
	#define IOR
	#define USE_SPECULAR
#endif
uniform vec3 diffuse;
uniform vec3 emissive;
uniform float roughness;
uniform float metalness;
uniform float opacity;
#ifdef IOR
	uniform float ior;
#endif
#ifdef USE_SPECULAR
	uniform float specularIntensity;
	uniform vec3 specularColor;
	#ifdef USE_SPECULAR_COLORMAP
		uniform sampler2D specularColorMap;
	#endif
	#ifdef USE_SPECULAR_INTENSITYMAP
		uniform sampler2D specularIntensityMap;
	#endif
#endif
#ifdef USE_CLEARCOAT
	uniform float clearcoat;
	uniform float clearcoatRoughness;
#endif
#ifdef USE_DISPERSION
	uniform float dispersion;
#endif
#ifdef USE_RETROREFLECTION
	uniform float retroreflectivity;
#endif
#ifdef USE_IRIDESCENCE
	uniform float iridescence;
	uniform float iridescenceIOR;
	uniform float iridescenceThicknessMinimum;
	uniform float iridescenceThicknessMaximum;
#endif
#ifdef USE_SHEEN
	uniform vec3 sheenColor;
	uniform float sheenRoughness;
	#ifdef USE_SHEEN_COLORMAP
		uniform sampler2D sheenColorMap;
	#endif
	#ifdef USE_SHEEN_ROUGHNESSMAP
		uniform sampler2D sheenRoughnessMap;
	#endif
#endif
#ifdef USE_ANISOTROPY
	uniform vec2 anisotropyVector;
	#ifdef USE_ANISOTROPYMAP
		uniform sampler2D anisotropyMap;
	#endif
#endif
varying vec3 vViewPosition;
#include <common>
#include <dithering_pars_fragment>
#include <color_pars_fragment>
#include <uv_pars_fragment>
#include <map_pars_fragment>
#include <alphamap_pars_fragment>
#include <alphatest_pars_fragment>
#include <alphahash_pars_fragment>
#include <aomap_pars_fragment>
#include <lightmap_pars_fragment>
#include <emissivemap_pars_fragment>
#include <iridescence_fragment>
#include <cube_uv_reflection_fragment>
#include <envmap_common_pars_fragment>
#include <envmap_physical_pars_fragment>
#include <fog_pars_fragment>
#include <lights_pars_begin>
#include <normal_pars_fragment>
#include <lights_physical_pars_fragment>
#include <transmission_pars_fragment>
#include <shadowmap_pars_fragment>
#include <bumpmap_pars_fragment>
#include <normalmap_pars_fragment>
#include <clearcoat_pars_fragment>
#include <iridescence_pars_fragment>
#include <roughnessmap_pars_fragment>
#include <metalnessmap_pars_fragment>
#include <logdepthbuf_pars_fragment>
#include <clipping_planes_pars_fragment>
void main() {
	vec4 diffuseColor = vec4( diffuse, opacity );
	#include <clipping_planes_fragment>
	ReflectedLight reflectedLight = ReflectedLight( vec3( 0.0 ), vec3( 0.0 ), vec3( 0.0 ), vec3( 0.0 ) );
	vec3 totalEmissiveRadiance = emissive;
	#include <logdepthbuf_fragment>
	#include <map_fragment>
	#include <color_fragment>
	#include <alphamap_fragment>
	#include <alphatest_fragment>
	#include <alphahash_fragment>
	#include <roughnessmap_fragment>
	#include <metalnessmap_fragment>
	#include <normal_fragment_begin>
	#include <normal_fragment_maps>
	#include <clearcoat_normal_fragment_begin>
	#include <clearcoat_normal_fragment_maps>
	#include <emissivemap_fragment>
	#include <lights_physical_fragment>
	#include <lights_fragment_begin>
	#include <lights_fragment_maps>
	#include <lights_fragment_end>
	#include <aomap_fragment>
	vec3 totalDiffuse = reflectedLight.directDiffuse + reflectedLight.indirectDiffuse;
	vec3 totalSpecular = reflectedLight.directSpecular + reflectedLight.indirectSpecular;
	#include <transmission_fragment>
	vec3 outgoingLight = totalDiffuse + totalSpecular + totalEmissiveRadiance;
	#ifdef USE_SHEEN
 
		outgoingLight = outgoingLight + sheenSpecularDirect + sheenSpecularIndirect;
 
 	#endif
	#ifdef USE_CLEARCOAT
		float dotNVcc = saturate( dot( geometryClearcoatNormal, geometryViewDir ) );
		vec3 Fcc = F_Schlick( material.clearcoatF0, material.clearcoatF90, dotNVcc );
		outgoingLight = outgoingLight * ( 1.0 - material.clearcoat * Fcc ) + ( clearcoatSpecularDirect + clearcoatSpecularIndirect ) * material.clearcoat;
	#endif
	#include <opaque_fragment>
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
	#include <fog_fragment>
	#include <premultiplied_alpha_fragment>
	#include <dithering_fragment>
}`,Fg=`#define TOON
varying vec3 vViewPosition;
#include <common>
#include <batching_pars_vertex>
#include <uv_pars_vertex>
#include <displacementmap_pars_vertex>
#include <color_pars_vertex>
#include <fog_pars_vertex>
#include <normal_pars_vertex>
#include <morphtarget_pars_vertex>
#include <skinning_pars_vertex>
#include <shadowmap_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>
void main() {
	#include <uv_vertex>
	#include <color_vertex>
	#include <morphinstance_vertex>
	#include <morphcolor_vertex>
	#include <batching_vertex>
	#include <beginnormal_vertex>
	#include <morphnormal_vertex>
	#include <skinbase_vertex>
	#include <skinnormal_vertex>
	#include <defaultnormal_vertex>
	#include <normal_vertex>
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <skinning_vertex>
	#include <displacementmap_vertex>
	#include <project_vertex>
	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
	vViewPosition = - mvPosition.xyz;
	#include <worldpos_vertex>
	#include <shadowmap_vertex>
	#include <fog_vertex>
}`,Og=`#define TOON
uniform vec3 diffuse;
uniform vec3 emissive;
uniform float opacity;
#include <common>
#include <dithering_pars_fragment>
#include <color_pars_fragment>
#include <uv_pars_fragment>
#include <map_pars_fragment>
#include <alphamap_pars_fragment>
#include <alphatest_pars_fragment>
#include <alphahash_pars_fragment>
#include <aomap_pars_fragment>
#include <lightmap_pars_fragment>
#include <emissivemap_pars_fragment>
#include <gradientmap_pars_fragment>
#include <fog_pars_fragment>
#include <bsdfs>
#include <lights_pars_begin>
#include <normal_pars_fragment>
#include <lights_toon_pars_fragment>
#include <shadowmap_pars_fragment>
#include <bumpmap_pars_fragment>
#include <normalmap_pars_fragment>
#include <logdepthbuf_pars_fragment>
#include <clipping_planes_pars_fragment>
void main() {
	vec4 diffuseColor = vec4( diffuse, opacity );
	#include <clipping_planes_fragment>
	ReflectedLight reflectedLight = ReflectedLight( vec3( 0.0 ), vec3( 0.0 ), vec3( 0.0 ), vec3( 0.0 ) );
	vec3 totalEmissiveRadiance = emissive;
	#include <logdepthbuf_fragment>
	#include <map_fragment>
	#include <color_fragment>
	#include <alphamap_fragment>
	#include <alphatest_fragment>
	#include <alphahash_fragment>
	#include <normal_fragment_begin>
	#include <normal_fragment_maps>
	#include <emissivemap_fragment>
	#include <lights_toon_fragment>
	#include <lights_fragment_begin>
	#include <lights_fragment_maps>
	#include <lights_fragment_end>
	#include <aomap_fragment>
	vec3 outgoingLight = reflectedLight.directDiffuse + reflectedLight.indirectDiffuse + totalEmissiveRadiance;
	#include <opaque_fragment>
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
	#include <fog_fragment>
	#include <premultiplied_alpha_fragment>
	#include <dithering_fragment>
}`,Bg=`uniform float size;
uniform float scale;
#include <common>
#include <color_pars_vertex>
#include <fog_pars_vertex>
#include <morphtarget_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>
#ifdef USE_POINTS_UV
	varying vec2 vUv;
	uniform mat3 uvTransform;
#endif
void main() {
	#ifdef USE_POINTS_UV
		vUv = ( uvTransform * vec3( uv, 1 ) ).xy;
	#endif
	#include <color_vertex>
	#include <morphinstance_vertex>
	#include <morphcolor_vertex>
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <project_vertex>
	gl_PointSize = size;
	#ifdef USE_SIZEATTENUATION
		bool isPerspective = isPerspectiveMatrix( projectionMatrix );
		if ( isPerspective ) gl_PointSize *= ( scale / - mvPosition.z );
	#endif
	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
	#include <worldpos_vertex>
	#include <fog_vertex>
}`,zg=`uniform vec3 diffuse;
uniform float opacity;
#include <common>
#include <color_pars_fragment>
#include <map_particle_pars_fragment>
#include <alphatest_pars_fragment>
#include <alphahash_pars_fragment>
#include <fog_pars_fragment>
#include <logdepthbuf_pars_fragment>
#include <clipping_planes_pars_fragment>
void main() {
	vec4 diffuseColor = vec4( diffuse, opacity );
	#include <clipping_planes_fragment>
	vec3 outgoingLight = vec3( 0.0 );
	#include <logdepthbuf_fragment>
	#include <map_particle_fragment>
	#include <color_fragment>
	#include <alphatest_fragment>
	#include <alphahash_fragment>
	outgoingLight = diffuseColor.rgb;
	#include <opaque_fragment>
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
	#include <fog_fragment>
	#include <premultiplied_alpha_fragment>
}`,Hg=`#include <common>
#include <batching_pars_vertex>
#include <fog_pars_vertex>
#include <morphtarget_pars_vertex>
#include <skinning_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <shadowmap_pars_vertex>
void main() {
	#include <batching_vertex>
	#include <beginnormal_vertex>
	#include <morphinstance_vertex>
	#include <morphnormal_vertex>
	#include <skinbase_vertex>
	#include <skinnormal_vertex>
	#include <defaultnormal_vertex>
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <skinning_vertex>
	#include <project_vertex>
	#include <logdepthbuf_vertex>
	#include <worldpos_vertex>
	#include <shadowmap_vertex>
	#include <fog_vertex>
}`,Vg=`uniform vec3 color;
uniform float opacity;
#include <common>
#include <fog_pars_fragment>
#include <bsdfs>
#include <lights_pars_begin>
#include <logdepthbuf_pars_fragment>
#include <shadowmap_pars_fragment>
#include <shadowmask_pars_fragment>
void main() {
	#include <logdepthbuf_fragment>
	gl_FragColor = vec4( color, opacity * ( 1.0 - getShadowMask() ) );
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
	#include <fog_fragment>
	#include <premultiplied_alpha_fragment>
}`,Gg=`uniform float rotation;
uniform vec2 center;
#include <common>
#include <uv_pars_vertex>
#include <fog_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>
void main() {
	#include <uv_vertex>
	vec4 mvPosition = modelViewMatrix[ 3 ];
	vec2 scale = vec2( length( modelMatrix[ 0 ].xyz ), length( modelMatrix[ 1 ].xyz ) );
	#ifndef USE_SIZEATTENUATION
		bool isPerspective = isPerspectiveMatrix( projectionMatrix );
		if ( isPerspective ) scale *= - mvPosition.z;
	#endif
	vec2 alignedPosition = ( position.xy - ( center - vec2( 0.5 ) ) ) * scale;
	vec2 rotatedPosition;
	rotatedPosition.x = cos( rotation ) * alignedPosition.x - sin( rotation ) * alignedPosition.y;
	rotatedPosition.y = sin( rotation ) * alignedPosition.x + cos( rotation ) * alignedPosition.y;
	mvPosition.xy += rotatedPosition;
	gl_Position = projectionMatrix * mvPosition;
	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
	#include <fog_vertex>
}`,Wg=`uniform vec3 diffuse;
uniform float opacity;
#include <common>
#include <uv_pars_fragment>
#include <map_pars_fragment>
#include <alphamap_pars_fragment>
#include <alphatest_pars_fragment>
#include <alphahash_pars_fragment>
#include <fog_pars_fragment>
#include <logdepthbuf_pars_fragment>
#include <clipping_planes_pars_fragment>
void main() {
	vec4 diffuseColor = vec4( diffuse, opacity );
	#include <clipping_planes_fragment>
	vec3 outgoingLight = vec3( 0.0 );
	#include <logdepthbuf_fragment>
	#include <map_fragment>
	#include <alphamap_fragment>
	#include <alphatest_fragment>
	#include <alphahash_fragment>
	outgoingLight = diffuseColor.rgb;
	#include <opaque_fragment>
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
	#include <fog_fragment>
}`,tt={alphahash_fragment:hm,alphahash_pars_fragment:dm,alphamap_fragment:fm,alphamap_pars_fragment:pm,alphatest_fragment:mm,alphatest_pars_fragment:gm,aomap_fragment:xm,aomap_pars_fragment:vm,batching_pars_vertex:_m,batching_vertex:ym,begin_vertex:bm,beginnormal_vertex:Mm,bsdfs:Sm,iridescence_fragment:wm,bumpmap_pars_fragment:Em,clipping_planes_fragment:Tm,clipping_planes_pars_fragment:Am,clipping_planes_pars_vertex:Rm,clipping_planes_vertex:Cm,color_fragment:Pm,color_pars_fragment:Im,color_pars_vertex:Lm,color_vertex:km,common:Dm,cube_uv_reflection_fragment:Nm,defaultnormal_vertex:Um,displacementmap_pars_vertex:Fm,displacementmap_vertex:Om,emissivemap_fragment:Bm,emissivemap_pars_fragment:zm,colorspace_fragment:Hm,colorspace_pars_fragment:Vm,envmap_fragment:Gm,envmap_common_pars_fragment:Wm,envmap_pars_fragment:Xm,envmap_pars_vertex:qm,envmap_physical_pars_fragment:i0,envmap_vertex:$m,fog_vertex:Ym,fog_pars_vertex:Zm,fog_fragment:Km,fog_pars_fragment:Jm,gradientmap_pars_fragment:jm,lightmap_pars_fragment:Qm,lights_lambert_fragment:e0,lights_lambert_pars_fragment:t0,lights_pars_begin:n0,lights_toon_fragment:r0,lights_toon_pars_fragment:s0,lights_phong_fragment:o0,lights_phong_pars_fragment:a0,lights_physical_fragment:l0,lights_physical_pars_fragment:c0,lights_fragment_begin:u0,lights_fragment_maps:h0,lights_fragment_end:d0,lightprobes_pars_fragment:f0,logdepthbuf_fragment:p0,logdepthbuf_pars_fragment:m0,logdepthbuf_pars_vertex:g0,logdepthbuf_vertex:x0,map_fragment:v0,map_pars_fragment:_0,map_particle_fragment:y0,map_particle_pars_fragment:b0,metalnessmap_fragment:M0,metalnessmap_pars_fragment:S0,morphinstance_vertex:w0,morphcolor_vertex:E0,morphnormal_vertex:T0,morphtarget_pars_vertex:A0,morphtarget_vertex:R0,normal_fragment_begin:C0,normal_fragment_maps:P0,normal_pars_fragment:I0,normal_pars_vertex:L0,normal_vertex:k0,normalmap_pars_fragment:D0,clearcoat_normal_fragment_begin:N0,clearcoat_normal_fragment_maps:U0,clearcoat_pars_fragment:F0,iridescence_pars_fragment:O0,opaque_fragment:B0,packing:z0,premultiplied_alpha_fragment:H0,project_vertex:V0,dithering_fragment:G0,dithering_pars_fragment:W0,roughnessmap_fragment:X0,roughnessmap_pars_fragment:q0,shadowmap_pars_fragment:$0,shadowmap_pars_vertex:Y0,shadowmap_vertex:Z0,shadowmask_pars_fragment:K0,skinbase_vertex:J0,skinning_pars_vertex:j0,skinning_vertex:Q0,skinnormal_vertex:eg,specularmap_fragment:tg,specularmap_pars_fragment:ng,tonemapping_fragment:ig,tonemapping_pars_fragment:rg,transmission_fragment:sg,transmission_pars_fragment:og,uv_pars_fragment:ag,uv_pars_vertex:lg,uv_vertex:cg,worldpos_vertex:ug,background_vert:hg,background_frag:dg,backgroundCube_vert:fg,backgroundCube_frag:pg,cube_vert:mg,cube_frag:gg,depth_vert:xg,depth_frag:vg,distance_vert:_g,distance_frag:yg,equirect_vert:bg,equirect_frag:Mg,linedashed_vert:Sg,linedashed_frag:wg,meshbasic_vert:Eg,meshbasic_frag:Tg,meshlambert_vert:Ag,meshlambert_frag:Rg,meshmatcap_vert:Cg,meshmatcap_frag:Pg,meshnormal_vert:Ig,meshnormal_frag:Lg,meshphong_vert:kg,meshphong_frag:Dg,meshphysical_vert:Ng,meshphysical_frag:Ug,meshtoon_vert:Fg,meshtoon_frag:Og,points_vert:Bg,points_frag:zg,shadow_vert:Hg,shadow_frag:Vg,sprite_vert:Gg,sprite_frag:Wg},Te={common:{diffuse:{value:new ze(16777215)},opacity:{value:1},map:{value:null},mapTransform:{value:new Ze},alphaMap:{value:null},alphaMapTransform:{value:new Ze},alphaTest:{value:0}},specularmap:{specularMap:{value:null},specularMapTransform:{value:new Ze}},envmap:{envMap:{value:null},envMapRotation:{value:new Ze},reflectivity:{value:1},ior:{value:1.5},refractionRatio:{value:.98},dfgLUT:{value:null}},aomap:{aoMap:{value:null},aoMapIntensity:{value:1},aoMapTransform:{value:new Ze}},lightmap:{lightMap:{value:null},lightMapIntensity:{value:1},lightMapTransform:{value:new Ze}},bumpmap:{bumpMap:{value:null},bumpMapTransform:{value:new Ze},bumpScale:{value:1}},normalmap:{normalMap:{value:null},normalMapTransform:{value:new Ze},normalScale:{value:new He(1,1)}},displacementmap:{displacementMap:{value:null},displacementMapTransform:{value:new Ze},displacementScale:{value:1},displacementBias:{value:0}},emissivemap:{emissiveMap:{value:null},emissiveMapTransform:{value:new Ze}},metalnessmap:{metalnessMap:{value:null},metalnessMapTransform:{value:new Ze}},roughnessmap:{roughnessMap:{value:null},roughnessMapTransform:{value:new Ze}},gradientmap:{gradientMap:{value:null}},fog:{fogDensity:{value:25e-5},fogNear:{value:1},fogFar:{value:2e3},fogColor:{value:new ze(16777215)}},lights:{ambientLightColor:{value:[]},lightProbe:{value:[]},sunLights:{value:[],properties:{direction:{},color:{}}},sunLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{}}},sunShadowMatrix:{value:[]},sunShadowCascade:{value:[]},directionalLights:{value:[],properties:{direction:{},color:{}}},directionalLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{}}},directionalShadowMatrix:{value:[]},spotLights:{value:[],properties:{color:{},position:{},direction:{},distance:{},coneCos:{},penumbraCos:{},decay:{}}},spotLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{}}},spotLightMap:{value:[]},spotLightMatrix:{value:[]},pointLights:{value:[],properties:{color:{},position:{},decay:{},distance:{}}},pointLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{},shadowCameraNear:{},shadowCameraFar:{}}},pointShadowMatrix:{value:[]},hemisphereLights:{value:[],properties:{direction:{},skyColor:{},groundColor:{}}},rectAreaLights:{value:[],properties:{color:{},position:{},width:{},height:{}}},ltc_1:{value:null},ltc_2:{value:null},probesSH:{value:null},probesMin:{value:new V},probesMax:{value:new V},probesResolution:{value:new V}},points:{diffuse:{value:new ze(16777215)},opacity:{value:1},size:{value:1},scale:{value:1},map:{value:null},alphaMap:{value:null},alphaMapTransform:{value:new Ze},alphaTest:{value:0},uvTransform:{value:new Ze}},sprite:{diffuse:{value:new ze(16777215)},opacity:{value:1},center:{value:new He(.5,.5)},rotation:{value:0},map:{value:null},mapTransform:{value:new Ze},alphaMap:{value:null},alphaMapTransform:{value:new Ze},alphaTest:{value:0}}},xi={basic:{uniforms:Sn([Te.common,Te.specularmap,Te.envmap,Te.aomap,Te.lightmap,Te.fog]),vertexShader:tt.meshbasic_vert,fragmentShader:tt.meshbasic_frag},lambert:{uniforms:Sn([Te.common,Te.specularmap,Te.envmap,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.fog,Te.lights,{emissive:{value:new ze(0)},envMapIntensity:{value:1}}]),vertexShader:tt.meshlambert_vert,fragmentShader:tt.meshlambert_frag},phong:{uniforms:Sn([Te.common,Te.specularmap,Te.envmap,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.fog,Te.lights,{emissive:{value:new ze(0)},specular:{value:new ze(1118481)},shininess:{value:30},envMapIntensity:{value:1}}]),vertexShader:tt.meshphong_vert,fragmentShader:tt.meshphong_frag},standard:{uniforms:Sn([Te.common,Te.envmap,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.roughnessmap,Te.metalnessmap,Te.fog,Te.lights,{emissive:{value:new ze(0)},roughness:{value:1},metalness:{value:0},envMapIntensity:{value:1}}]),vertexShader:tt.meshphysical_vert,fragmentShader:tt.meshphysical_frag},toon:{uniforms:Sn([Te.common,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.gradientmap,Te.fog,Te.lights,{emissive:{value:new ze(0)}}]),vertexShader:tt.meshtoon_vert,fragmentShader:tt.meshtoon_frag},matcap:{uniforms:Sn([Te.common,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.fog,{matcap:{value:null}}]),vertexShader:tt.meshmatcap_vert,fragmentShader:tt.meshmatcap_frag},points:{uniforms:Sn([Te.points,Te.fog]),vertexShader:tt.points_vert,fragmentShader:tt.points_frag},dashed:{uniforms:Sn([Te.common,Te.fog,{scale:{value:1},dashSize:{value:1},totalSize:{value:2}}]),vertexShader:tt.linedashed_vert,fragmentShader:tt.linedashed_frag},depth:{uniforms:Sn([Te.common,Te.displacementmap]),vertexShader:tt.depth_vert,fragmentShader:tt.depth_frag},normal:{uniforms:Sn([Te.common,Te.bumpmap,Te.normalmap,Te.displacementmap,{opacity:{value:1}}]),vertexShader:tt.meshnormal_vert,fragmentShader:tt.meshnormal_frag},sprite:{uniforms:Sn([Te.sprite,Te.fog]),vertexShader:tt.sprite_vert,fragmentShader:tt.sprite_frag},background:{uniforms:{uvTransform:{value:new Ze},t2D:{value:null},backgroundIntensity:{value:1}},vertexShader:tt.background_vert,fragmentShader:tt.background_frag},backgroundCube:{uniforms:{envMap:{value:null},backgroundBlurriness:{value:0},backgroundIntensity:{value:1},backgroundRotation:{value:new Ze}},vertexShader:tt.backgroundCube_vert,fragmentShader:tt.backgroundCube_frag},cube:{uniforms:{tCube:{value:null},tFlip:{value:-1},opacity:{value:1}},vertexShader:tt.cube_vert,fragmentShader:tt.cube_frag},equirect:{uniforms:{tEquirect:{value:null}},vertexShader:tt.equirect_vert,fragmentShader:tt.equirect_frag},distance:{uniforms:Sn([Te.common,Te.displacementmap,{referencePosition:{value:new V},nearDistance:{value:1},farDistance:{value:1e3}}]),vertexShader:tt.distance_vert,fragmentShader:tt.distance_frag},shadow:{uniforms:Sn([Te.lights,Te.fog,{color:{value:new ze(0)},opacity:{value:1}}]),vertexShader:tt.shadow_vert,fragmentShader:tt.shadow_frag}};xi.physical={uniforms:Sn([xi.standard.uniforms,{clearcoat:{value:0},clearcoatMap:{value:null},clearcoatMapTransform:{value:new Ze},clearcoatNormalMap:{value:null},clearcoatNormalMapTransform:{value:new Ze},clearcoatNormalScale:{value:new He(1,1)},clearcoatRoughness:{value:0},clearcoatRoughnessMap:{value:null},clearcoatRoughnessMapTransform:{value:new Ze},dispersion:{value:0},retroreflectivity:{value:0},iridescence:{value:0},iridescenceMap:{value:null},iridescenceMapTransform:{value:new Ze},iridescenceIOR:{value:1.3},iridescenceThicknessMinimum:{value:100},iridescenceThicknessMaximum:{value:400},iridescenceThicknessMap:{value:null},iridescenceThicknessMapTransform:{value:new Ze},sheen:{value:0},sheenColor:{value:new ze(0)},sheenColorMap:{value:null},sheenColorMapTransform:{value:new Ze},sheenRoughness:{value:1},sheenRoughnessMap:{value:null},sheenRoughnessMapTransform:{value:new Ze},transmission:{value:0},transmissionMap:{value:null},transmissionMapTransform:{value:new Ze},transmissionSamplerSize:{value:new He},transmissionSamplerMap:{value:null},thickness:{value:0},thicknessMap:{value:null},thicknessMapTransform:{value:new Ze},attenuationDistance:{value:0},attenuationColor:{value:new ze(0)},specularColor:{value:new ze(1,1,1)},specularColorMap:{value:null},specularColorMapTransform:{value:new Ze},specularIntensity:{value:1},specularIntensityMap:{value:null},specularIntensityMapTransform:{value:new Ze},anisotropyVector:{value:new He},anisotropyMap:{value:null},anisotropyMapTransform:{value:new Ze}}]),vertexShader:tt.meshphysical_vert,fragmentShader:tt.meshphysical_frag};var _l={r:0,b:0,g:0},Xg=new It,Hd=new Ze;Hd.set(-1,0,0,0,1,0,0,0,1);function qg(n,e,t,i,r,s){let o=new ze(0),a=r===!0?0:1,c,l,h=null,m=0,u=null;function g(S){let P=S.isScene===!0?S.background:null;if(P&&P.isTexture){let w=S.backgroundBlurriness>0;P=e.get(P,w)}return P}function _(S){let P=!1,w=g(S);w===null?f(o,a):w&&w.isColor&&(f(w,1),P=!0);let T=n.xr.getEnvironmentBlendMode();T==="additive"?t.buffers.color.setClear(0,0,0,1,s):T==="alpha-blend"&&t.buffers.color.setClear(0,0,0,0,s),(n.autoClear||P)&&(t.buffers.depth.setTest(!0),t.buffers.depth.setMask(!0),t.buffers.color.setMask(!0),n.clear(n.autoClearColor,n.autoClearDepth,n.autoClearStencil))}function x(S,P){let w=g(P);w&&(w.isCubeTexture||w.mapping===to)?(l===void 0&&(l=new xt(new Ke(1,1,1),new On({name:"BackgroundCubeMaterial",uniforms:gr(xi.backgroundCube.uniforms),vertexShader:xi.backgroundCube.vertexShader,fragmentShader:xi.backgroundCube.fragmentShader,side:Rn,depthTest:!1,depthWrite:!1,fog:!1,allowOverride:!1})),l.geometry.deleteAttribute("normal"),l.geometry.deleteAttribute("uv"),l.onBeforeRender=function(T,A,D){this.matrixWorld.copyPosition(D.matrixWorld)},Object.defineProperty(l.material,"envMap",{get:function(){return this.uniforms.envMap.value}}),i.update(l)),l.material.uniforms.envMap.value=w,l.material.uniforms.backgroundBlurriness.value=P.backgroundBlurriness,l.material.uniforms.backgroundIntensity.value=P.backgroundIntensity,l.material.uniforms.backgroundRotation.value.setFromMatrix4(Xg.makeRotationFromEuler(P.backgroundRotation)).transpose(),w.isCubeTexture&&w.isRenderTargetTexture===!1&&l.material.uniforms.backgroundRotation.value.premultiply(Hd),l.material.toneMapped=lt.getTransfer(w.colorSpace)!==Mt,(h!==w||m!==w.version||u!==n.toneMapping)&&(l.material.needsUpdate=!0,h=w,m=w.version,u=n.toneMapping),l.layers.enableAll(),S.unshift(l,l.geometry,l.material,0,0,null)):w&&w.isTexture&&(c===void 0&&(c=new xt(new Gi(2,2),new On({name:"BackgroundMaterial",uniforms:gr(xi.background.uniforms),vertexShader:xi.background.vertexShader,fragmentShader:xi.background.fragmentShader,side:Zi,depthTest:!1,depthWrite:!1,fog:!1,allowOverride:!1})),c.geometry.deleteAttribute("normal"),Object.defineProperty(c.material,"map",{get:function(){return this.uniforms.t2D.value}}),i.update(c)),c.material.uniforms.t2D.value=w,c.material.uniforms.backgroundIntensity.value=P.backgroundIntensity,c.material.toneMapped=lt.getTransfer(w.colorSpace)!==Mt,w.matrixAutoUpdate===!0&&w.updateMatrix(),c.material.uniforms.uvTransform.value.copy(w.matrix),(h!==w||m!==w.version||u!==n.toneMapping)&&(c.material.needsUpdate=!0,h=w,m=w.version,u=n.toneMapping),c.layers.enableAll(),S.unshift(c,c.geometry,c.material,0,0,null))}function f(S,P){S.getRGB(_l,jc(n)),t.buffers.color.setClear(_l.r,_l.g,_l.b,P,s)}function d(){l!==void 0&&(l.geometry.dispose(),l.material.dispose(),l=void 0),c!==void 0&&(c.geometry.dispose(),c.material.dispose(),c=void 0)}return{getClearColor:function(){return o},setClearColor:function(S,P=1){o.set(S),a=P,f(o,a)},getClearAlpha:function(){return a},setClearAlpha:function(S){a=S,f(o,a)},render:_,addToRenderList:x,dispose:d}}function $g(n,e){let t=n.getParameter(n.MAX_VERTEX_ATTRIBS),i={},r=u(null),s=r,o=!1;function a(C,k,L,U,F){let H=!1,q=m(C,U,L,k);s!==q&&(s=q,l(s.object)),H=g(C,U,L,F),H&&_(C,U,L,F),F!==null&&e.update(F,n.ELEMENT_ARRAY_BUFFER),(H||o)&&(o=!1,w(C,k,L,U),F!==null&&n.bindBuffer(n.ELEMENT_ARRAY_BUFFER,e.get(F).buffer))}function c(){return n.createVertexArray()}function l(C){return n.bindVertexArray(C)}function h(C){return n.deleteVertexArray(C)}function m(C,k,L,U){let F=U.wireframe===!0,H=i[k.id];H===void 0&&(H={},i[k.id]=H);let q=C.isInstancedMesh===!0?C.id:0,ee=H[q];ee===void 0&&(ee={},H[q]=ee);let J=ee[L.id];J===void 0&&(J={},ee[L.id]=J);let ne=J[F];return ne===void 0&&(ne=u(c()),J[F]=ne),ne}function u(C){let k=[],L=[],U=[];for(let F=0;F<t;F++)k[F]=0,L[F]=0,U[F]=0;return{geometry:null,program:null,wireframe:!1,newAttributes:k,enabledAttributes:L,attributeDivisors:U,object:C,attributes:{},index:null}}function g(C,k,L,U){let F=s.attributes,H=k.attributes,q=0,ee=L.getAttributes();for(let J in ee)if(ee[J].location>=0){let ce=F[J],pe=H[J];if(pe===void 0&&(J==="instanceMatrix"&&C.instanceMatrix&&(pe=C.instanceMatrix),J==="instanceColor"&&C.instanceColor&&(pe=C.instanceColor)),ce===void 0||ce.attribute!==pe||pe&&ce.data!==pe.data)return!0;q++}return s.attributesNum!==q||s.index!==U}function _(C,k,L,U){let F={},H=k.attributes,q=0,ee=L.getAttributes();for(let J in ee)if(ee[J].location>=0){let ce=H[J];ce===void 0&&(J==="instanceMatrix"&&C.instanceMatrix&&(ce=C.instanceMatrix),J==="instanceColor"&&C.instanceColor&&(ce=C.instanceColor));let pe={};pe.attribute=ce,ce&&ce.data&&(pe.data=ce.data),F[J]=pe,q++}s.attributes=F,s.attributesNum=q,s.index=U}function x(){let C=s.newAttributes;for(let k=0,L=C.length;k<L;k++)C[k]=0}function f(C){d(C,0)}function d(C,k){let L=s.newAttributes,U=s.enabledAttributes,F=s.attributeDivisors;L[C]=1,U[C]===0&&(n.enableVertexAttribArray(C),U[C]=1),F[C]!==k&&(n.vertexAttribDivisor(C,k),F[C]=k)}function S(){let C=s.newAttributes,k=s.enabledAttributes;for(let L=0,U=k.length;L<U;L++)k[L]!==C[L]&&(n.disableVertexAttribArray(L),k[L]=0)}function P(C,k,L,U,F,H,q){q===!0?n.vertexAttribIPointer(C,k,L,F,H):n.vertexAttribPointer(C,k,L,U,F,H)}function w(C,k,L,U){x();let F=U.attributes,H=L.getAttributes(),q=k.defaultAttributeValues;for(let ee in H){let J=H[ee];if(J.location>=0){let ne=F[ee];if(ne===void 0&&(ee==="instanceMatrix"&&C.instanceMatrix&&(ne=C.instanceMatrix),ee==="instanceColor"&&C.instanceColor&&(ne=C.instanceColor)),ne!==void 0){let ce=ne.normalized,pe=ne.itemSize,me=e.get(ne);if(me===void 0)continue;let qe=me.buffer,Le=me.type,Je=me.bytesPerElement,se=Le===n.INT||Le===n.UNSIGNED_INT||ne.gpuType===Da;if(ne.isInterleavedBufferAttribute){let le=ne.data,Se=le.stride,ke=ne.offset;if(le.isInstancedInterleavedBuffer){for(let ye=0;ye<J.locationSize;ye++)d(J.location+ye,le.meshPerAttribute);C.isInstancedMesh!==!0&&U._maxInstanceCount===void 0&&(U._maxInstanceCount=le.meshPerAttribute*le.count)}else for(let ye=0;ye<J.locationSize;ye++)f(J.location+ye);n.bindBuffer(n.ARRAY_BUFFER,qe);for(let ye=0;ye<J.locationSize;ye++)P(J.location+ye,pe/J.locationSize,Le,ce,Se*Je,(ke+pe/J.locationSize*ye)*Je,se)}else{if(ne.isInstancedBufferAttribute){for(let le=0;le<J.locationSize;le++)d(J.location+le,ne.meshPerAttribute);C.isInstancedMesh!==!0&&U._maxInstanceCount===void 0&&(U._maxInstanceCount=ne.meshPerAttribute*ne.count)}else for(let le=0;le<J.locationSize;le++)f(J.location+le);n.bindBuffer(n.ARRAY_BUFFER,qe);for(let le=0;le<J.locationSize;le++)P(J.location+le,pe/J.locationSize,Le,ce,pe*Je,pe/J.locationSize*le*Je,se)}}else if(q!==void 0){let ce=q[ee];if(ce!==void 0)switch(ce.length){case 2:n.vertexAttrib2fv(J.location,ce);break;case 3:n.vertexAttrib3fv(J.location,ce);break;case 4:n.vertexAttrib4fv(J.location,ce);break;default:n.vertexAttrib1fv(J.location,ce)}}}}S()}function T(){M();for(let C in i){let k=i[C];for(let L in k){let U=k[L];for(let F in U){let H=U[F];for(let q in H)h(H[q].object),delete H[q];delete U[F]}}delete i[C]}}function A(C){if(i[C.id]===void 0)return;let k=i[C.id];for(let L in k){let U=k[L];for(let F in U){let H=U[F];for(let q in H)h(H[q].object),delete H[q];delete U[F]}}delete i[C.id]}function D(C){for(let k in i){let L=i[k];for(let U in L){let F=L[U];if(F[C.id]===void 0)continue;let H=F[C.id];for(let q in H)h(H[q].object),delete H[q];delete F[C.id]}}}function p(C){for(let k in i){let L=i[k],U=C.isInstancedMesh===!0?C.id:0,F=L[U];if(F!==void 0){for(let H in F){let q=F[H];for(let ee in q)h(q[ee].object),delete q[ee];delete F[H]}delete L[U],Object.keys(L).length===0&&delete i[k]}}}function M(){y(),o=!0,s!==r&&(s=r,l(s.object))}function y(){r.geometry=null,r.program=null,r.wireframe=!1}return{setup:a,reset:M,resetDefaultState:y,dispose:T,releaseStatesOfGeometry:A,releaseStatesOfObject:p,releaseStatesOfProgram:D,initAttributes:x,enableAttribute:f,disableUnusedAttributes:S}}function Yg(n,e,t){let i;function r(c){i=c}function s(c,l){n.drawArrays(i,c,l),t.update(l,i,1)}function o(c,l,h){h!==0&&(n.drawArraysInstanced(i,c,l,h),t.update(l,i,h))}function a(c,l,h){if(h===0)return;e.get("WEBGL_multi_draw").multiDrawArraysWEBGL(i,c,0,l,0,h);let u=0;for(let g=0;g<h;g++)u+=l[g];t.update(u,i,1)}this.setMode=r,this.render=s,this.renderInstances=o,this.renderMultiDraw=a}function Zg(n,e,t,i){let r;function s(){if(r!==void 0)return r;if(e.has("EXT_texture_filter_anisotropic")===!0){let D=e.get("EXT_texture_filter_anisotropic");r=n.getParameter(D.MAX_TEXTURE_MAX_ANISOTROPY_EXT)}else r=0;return r}function o(D){return!(D!==$n&&i.convert(D)!==n.getParameter(n.IMPLEMENTATION_COLOR_READ_FORMAT))}function a(D){let p=D===si&&(e.has("EXT_color_buffer_half_float")||e.has("EXT_color_buffer_float"));return!(D!==Ln&&D!==ri&&!p&&i.convert(D)!==n.getParameter(n.IMPLEMENTATION_COLOR_READ_TYPE))}function c(D){if(D==="highp"){if(n.getShaderPrecisionFormat(n.VERTEX_SHADER,n.HIGH_FLOAT).precision>0&&n.getShaderPrecisionFormat(n.FRAGMENT_SHADER,n.HIGH_FLOAT).precision>0)return"highp";D="mediump"}return D==="mediump"&&n.getShaderPrecisionFormat(n.VERTEX_SHADER,n.MEDIUM_FLOAT).precision>0&&n.getShaderPrecisionFormat(n.FRAGMENT_SHADER,n.MEDIUM_FLOAT).precision>0?"mediump":"lowp"}let l=t.precision!==void 0?t.precision:"highp",h=c(l);h!==l&&(Ge("WebGLRenderer:",l,"not supported, using",h,"instead."),l=h);let m=t.logarithmicDepthBuffer===!0,u=t.reversedDepthBuffer===!0&&e.has("EXT_clip_control");t.reversedDepthBuffer===!0&&u===!1&&Ge("WebGLRenderer: Unable to use reversed depth buffer due to missing EXT_clip_control extension. Fallback to default depth buffer.");let g=n.getParameter(n.MAX_TEXTURE_IMAGE_UNITS),_=n.getParameter(n.MAX_VERTEX_TEXTURE_IMAGE_UNITS),x=n.getParameter(n.MAX_TEXTURE_SIZE),f=n.getParameter(n.MAX_CUBE_MAP_TEXTURE_SIZE),d=n.getParameter(n.MAX_VERTEX_ATTRIBS),S=n.getParameter(n.MAX_VERTEX_UNIFORM_VECTORS),P=n.getParameter(n.MAX_VARYING_VECTORS),w=n.getParameter(n.MAX_FRAGMENT_UNIFORM_VECTORS),T=n.getParameter(n.MAX_SAMPLES),A=n.getParameter(n.SAMPLES);return{isWebGL2:!0,getMaxAnisotropy:s,getMaxPrecision:c,textureFormatReadable:o,textureTypeReadable:a,precision:l,logarithmicDepthBuffer:m,reversedDepthBuffer:u,maxTextures:g,maxVertexTextures:_,maxTextureSize:x,maxCubemapSize:f,maxAttributes:d,maxVertexUniforms:S,maxVaryings:P,maxFragmentUniforms:w,maxSamples:T,samples:A}}function Kg(n){let e=this,t=null,i=0,r=!1,s=!1,o=new Un,a=new Ze,c={value:null,needsUpdate:!1};this.uniform=c,this.numPlanes=0,this.numIntersection=0,this.init=function(m,u){let g=m.length!==0||u||i!==0||r;return r=u,i=m.length,g},this.beginShadows=function(){s=!0,h(null)},this.endShadows=function(){s=!1},this.setGlobalState=function(m,u){t=h(m,u,0)},this.setState=function(m,u,g){let _=m.clippingPlanes,x=m.clipIntersection,f=m.clipShadows,d=n.get(m);if(!r||_===null||_.length===0||s&&!f)s?h(null):l();else{let S=s?0:i,P=S*4,w=d.clippingState||null;c.value=w,w=h(_,u,P,g);for(let T=0;T!==P;++T)w[T]=t[T];d.clippingState=w,this.numIntersection=x?this.numPlanes:0,this.numPlanes+=S}};function l(){c.value!==t&&(c.value=t,c.needsUpdate=i>0),e.numPlanes=i,e.numIntersection=0}function h(m,u,g,_){let x=m!==null?m.length:0,f=null;if(x!==0){if(f=c.value,_!==!0||f===null){let d=g+x*4,S=u.matrixWorldInverse;a.getNormalMatrix(S),(f===null||f.length<d)&&(f=new Float32Array(d));for(let P=0,w=g;P!==x;++P,w+=4)o.copy(m[P]).applyMatrix4(S,a),o.normal.toArray(f,w),f[w+3]=o.constant}c.value=f,c.needsUpdate=!0}return e.numPlanes=x,e.numIntersection=0,f}}var fs=4,Jg=6,jg=20,Qg=256,co=new rs,_d=new ze,nu=null,iu=0,ru=0,su=!1,ex=new V,xr=new V,bl=class{constructor(e){this._renderer=e,this._pingPongRenderTarget=null,this._lodMax=0,this._cubeSize=0,this._sizeLods=[],this._lodMeshes=[],this._backgroundBox=null,this._cubemapMaterial=null,this._equirectMaterial=null,this._blurMaterial=null,this._ggxMaterial=null}fromScene(e,t=0,i=.1,r=100,s={}){let{size:o=256,position:a=ex}=s;nu=this._renderer.getRenderTarget(),iu=this._renderer.getActiveCubeFace(),ru=this._renderer.getActiveMipmapLevel(),su=this._renderer.xr.enabled,this._renderer.xr.enabled=!1,this._setSize(o);let c=this._allocateTargets();return c.depthBuffer=!0,this._sceneToCubeUV(e,i,r,c,a),t>0&&this._blur(c,0,0,t),this._applyPMREM(c),this._cleanup(c),c}fromEquirectangular(e,t=null){return this._fromTexture(e,t)}fromCubemap(e,t=null){return this._fromTexture(e,t)}compileCubemapShader(){this._cubemapMaterial===null&&(this._cubemapMaterial=Md(),this._compileMaterial(this._cubemapMaterial))}compileEquirectangularShader(){this._equirectMaterial===null&&(this._equirectMaterial=bd(),this._compileMaterial(this._equirectMaterial))}dispose(){this._dispose(),this._cubemapMaterial!==null&&this._cubemapMaterial.dispose(),this._equirectMaterial!==null&&this._equirectMaterial.dispose(),this._backgroundBox!==null&&(this._backgroundBox.geometry.dispose(),this._backgroundBox.material.dispose())}_setSize(e){this._lodMax=Math.floor(Math.log2(e)),this._cubeSize=Math.pow(2,this._lodMax)}_dispose(){this._blurMaterial!==null&&this._blurMaterial.dispose(),this._ggxMaterial!==null&&this._ggxMaterial.dispose(),this._pingPongRenderTarget!==null&&this._pingPongRenderTarget.dispose();for(let e=0;e<this._lodMeshes.length;e++)this._lodMeshes[e].geometry.dispose()}_cleanup(e){this._renderer.setRenderTarget(nu,iu,ru),this._renderer.xr.enabled=su,e.scissorTest=!1,ds(e,0,0,e.width,e.height)}_fromTexture(e,t){e.mapping===Ki||e.mapping===mr?this._setSize(e.image.length===0?16:e.image[0].width||e.image[0].image.width):this._setSize(e.image.width/4),nu=this._renderer.getRenderTarget(),iu=this._renderer.getActiveCubeFace(),ru=this._renderer.getActiveMipmapLevel(),su=this._renderer.xr.enabled,this._renderer.xr.enabled=!1;let i=t||this._allocateTargets();return this._textureToCubeUV(e,i),this._applyPMREM(i),this._cleanup(i),i}_allocateTargets(){let e=3*Math.max(this._cubeSize,112),t=4*this._cubeSize,i={magFilter:gn,minFilter:gn,generateMipmaps:!1,type:si,format:$n,colorSpace:Ls,depthBuffer:!1},r=yd(e,t,i);if(this._pingPongRenderTarget===null||this._pingPongRenderTarget.width!==e||this._pingPongRenderTarget.height!==t){this._pingPongRenderTarget!==null&&this._dispose(),this._pingPongRenderTarget=yd(e,t,i);let{_lodMax:s}=this;({lodMeshes:this._lodMeshes,sizeLods:this._sizeLods}=tx(s)),this._blurMaterial=ix(s,e,t),this._ggxMaterial=nx(s,e,t)}return r}_compileMaterial(e){let t=new xt(new Xt,e);this._renderer.compile(t,co)}_sceneToCubeUV(e,t,i,r,s){let c=new mn(90,1,t,i),l=[1,-1,1,1,1,1],h=[1,1,1,-1,-1,-1],m=this._renderer,u=m.autoClear,g=m.toneMapping;m.getClearColor(_d),m.toneMapping=ni,m.autoClear=!1,m.state.buffers.depth.getReversed()&&(m.setRenderTarget(r),m.clearDepth(),m.setRenderTarget(null)),this._backgroundBox===null&&(this._backgroundBox=new xt(new Ke,new on({name:"PMREM.Background",side:Rn,depthWrite:!1,depthTest:!1})));let x=this._backgroundBox,f=x.material,d=!1,S=e.background;S?S.isColor&&(f.color.copy(S),e.background=null,d=!0):(f.color.copy(_d),d=!0);for(let P=0;P<6;P++){let w=P%3;w===0?(c.up.set(0,l[P],0),c.position.set(s.x,s.y,s.z),c.lookAt(s.x+h[P],s.y,s.z)):w===1?(c.up.set(0,0,l[P]),c.position.set(s.x,s.y,s.z),c.lookAt(s.x,s.y+h[P],s.z)):(c.up.set(0,l[P],0),c.position.set(s.x,s.y,s.z),c.lookAt(s.x,s.y,s.z+h[P]));let T=this._cubeSize;ds(r,w*T,P>2?T:0,T,T),m.setRenderTarget(r),d&&m.render(x,c),m.render(e,c)}m.toneMapping=g,m.autoClear=u,e.background=S}_textureToCubeUV(e,t){let i=this._renderer,r=e.mapping===Ki||e.mapping===mr;r?(this._cubemapMaterial===null&&(this._cubemapMaterial=Md()),this._cubemapMaterial.uniforms.flipEnvMap.value=e.isRenderTargetTexture===!1?-1:1):this._equirectMaterial===null&&(this._equirectMaterial=bd());let s=r?this._cubemapMaterial:this._equirectMaterial,o=this._lodMeshes[0];o.material=s;let a=s.uniforms;a.envMap.value=e;let c=this._cubeSize;ds(t,0,0,3*c,2*c),i.setRenderTarget(t),i.render(o,co)}_applyPMREM(e){let t=this._renderer,i=t.autoClear;t.autoClear=!1;let r=this._lodMeshes.length;for(let s=1;s<r;s++)this._applyGGXFilter(e,s-1,s);t.autoClear=i}_applyGGXFilter(e,t,i){let r=this._renderer,s=this._pingPongRenderTarget,o=this._ggxMaterial,a=this._lodMeshes[i];a.material=o;let c=o.uniforms,l=i/(this._lodMeshes.length-1),h=t/(this._lodMeshes.length-1),m=Math.sqrt(l*l-h*h),u=l*1.25,g=m*u,{_lodMax:_}=this,x=this._sizeLods[i],f=3*x*(i>_-fs?i-_+fs:0),d=4*(this._cubeSize-x);c.envMap.value=e.texture,c.roughness.value=g,c.mipInt.value=_-t,ds(s,f,d,3*x,2*x),r.setRenderTarget(s),r.render(a,co),c.envMap.value=s.texture,c.roughness.value=0,c.mipInt.value=_-i,ds(e,f,d,3*x,2*x),r.setRenderTarget(e),r.render(a,co)}_blur(e,t,i,r){let s=this._pingPongRenderTarget,o=Math.min(r,Math.PI)/Math.SQRT2;this._blurPass(e,s,t,i,o),this._blurPass(s,e,i,i,o)}_blurPass(e,t,i,r,s){let o=this._renderer,a=this._blurMaterial,c=this._lodMeshes[r];c.material=a;let l=a.uniforms;l.envMap.value=e.texture,l.sigma.value=s,l.mipInt.value=this._lodMax-i;let h=this._sizeLods[r],m=3*h*(r>this._lodMax-fs?r-this._lodMax+fs:0),u=4*(this._cubeSize-h);ds(t,m,u,3*h,2*h),o.setRenderTarget(t),o.render(c,co)}};function tx(n){let e=[],t=[],i=n,r=n-fs+1+Jg;for(let s=0;s<r;s++){let o=Math.pow(2,i);e.push(o);let a=1/(o-2),c=-a,l=1+a,h=[c,c,l,c,l,l,c,c,l,l,c,l],m=6,u=6,g=3,_=new Float32Array(g*u*m),x=new Float32Array(g*u*m);for(let d=0;d<m;d++){let S=d%3*2/3-1,P=d>2?0:-1,w=[S,P,0,S+2/3,P,0,S+2/3,P+1,0,S,P,0,S+2/3,P+1,0,S,P+1,0];_.set(w,g*u*d);for(let T=0;T<u;T++){let A=h[T*2]*2-1,D=h[T*2+1]*2-1;d===0?xr.set(1,D,A):d===1?xr.set(-A,1,-D):d===2?xr.set(-A,D,1):d===3?xr.set(-1,D,-A):d===4?xr.set(-A,-1,D):xr.set(A,D,-1),xr.toArray(x,(d*u+T)*g)}}let f=new Xt;f.setAttribute("position",new Pn(_,g)),f.setAttribute("outputDirection",new Pn(x,g)),t.push(new xt(f,null)),i>fs&&i--}return{lodMeshes:t,sizeLods:e}}function yd(n,e,t){let i=new In(n,e,t);return i.texture.mapping=to,i.texture.name="PMREM.cubeUv",i.scissorTest=!0,i}function ds(n,e,t,i,r){n.viewport.set(e,t,i,r),n.scissor.set(e,t,i,r)}function nx(n,e,t){return new On({name:"PMREMGGXConvolution",defines:{GGX_SAMPLES:Qg,CUBEUV_TEXEL_WIDTH:1/e,CUBEUV_TEXEL_HEIGHT:1/t,CUBEUV_MAX_MIP:`${n}.0`},uniforms:{envMap:{value:null},roughness:{value:0},mipInt:{value:0}},vertexShader:wl(),fragmentShader:`

			precision highp float;
			precision highp int;

			varying vec3 vOutputDirection;

			uniform sampler2D envMap;
			uniform float roughness;
			uniform float mipInt;

			#define ENVMAP_TYPE_CUBE_UV
			#include <cube_uv_reflection_fragment>

			#define PI 3.14159265359

			// Van der Corput radical inverse
			float radicalInverse_VdC(uint bits) {
				bits = (bits << 16u) | (bits >> 16u);
				bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
				bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
				bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
				bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
				return float(bits) * 2.3283064365386963e-10; // / 0x100000000
			}

			// Hammersley sequence
			vec2 hammersley(uint i, uint N) {
				return vec2(float(i) / float(N), radicalInverse_VdC(i));
			}

			// GGX VNDF importance sampling (Eric Heitz 2018)
			// "Sampling the GGX Distribution of Visible Normals"
			// https://jcgt.org/published/0007/04/01/
			vec3 importanceSampleGGX_VNDF(vec2 Xi, vec3 V, float roughness) {
				float alpha = roughness * roughness;

				// Section 4.1: Orthonormal basis
				vec3 T1 = vec3(1.0, 0.0, 0.0);
				vec3 T2 = cross(V, T1);

				// Section 4.2: Parameterization of projected area
				float r = sqrt(Xi.x);
				float phi = 2.0 * PI * Xi.y;
				float t1 = r * cos(phi);
				float t2 = r * sin(phi);
				float s = 0.5 * (1.0 + V.z);
				t2 = (1.0 - s) * sqrt(1.0 - t1 * t1) + s * t2;

				// Section 4.3: Reprojection onto hemisphere
				vec3 Nh = t1 * T1 + t2 * T2 + sqrt(max(0.0, 1.0 - t1 * t1 - t2 * t2)) * V;

				// Section 3.4: Transform back to ellipsoid configuration
				return normalize(vec3(alpha * Nh.x, alpha * Nh.y, max(0.0, Nh.z)));
			}

			void main() {
				vec3 N = normalize(vOutputDirection);
				vec3 V = N; // Assume view direction equals normal for pre-filtering

				vec3 prefilteredColor = vec3(0.0);
				float totalWeight = 0.0;

				// For very low roughness, just sample the environment directly
				if (roughness < 0.001) {
					gl_FragColor = vec4(bilinearCubeUV(envMap, N, mipInt), 1.0);
					return;
				}

				// Tangent space basis for VNDF sampling
				vec3 up = abs(N.z) < 0.999 ? vec3(0.0, 0.0, 1.0) : vec3(1.0, 0.0, 0.0);
				vec3 tangent = normalize(cross(up, N));
				vec3 bitangent = cross(N, tangent);

				for(uint i = 0u; i < uint(GGX_SAMPLES); i++) {
					vec2 Xi = hammersley(i, uint(GGX_SAMPLES));

					// For PMREM, V = N, so in tangent space V is always (0, 0, 1)
					vec3 H_tangent = importanceSampleGGX_VNDF(Xi, vec3(0.0, 0.0, 1.0), roughness);

					// Transform H back to world space
					vec3 H = normalize(tangent * H_tangent.x + bitangent * H_tangent.y + N * H_tangent.z);
					vec3 L = normalize(2.0 * dot(V, H) * H - V);

					float NdotL = max(dot(N, L), 0.0);

					if(NdotL > 0.0) {
						// Sample environment at fixed mip level
						// VNDF importance sampling handles the distribution filtering
						vec3 sampleColor = bilinearCubeUV(envMap, L, mipInt);

						// Weight by NdotL for the split-sum approximation
						// VNDF PDF naturally accounts for the visible microfacet distribution
						prefilteredColor += sampleColor * NdotL;
						totalWeight += NdotL;
					}
				}

				if (totalWeight > 0.0) {
					prefilteredColor = prefilteredColor / totalWeight;
				}

				gl_FragColor = vec4(prefilteredColor, 1.0);
			}
		`,blending:mi,depthTest:!1,depthWrite:!1})}function ix(n,e,t){return new On({name:"SphericalGaussianBlur",defines:{SAMPLES:jg,CUBEUV_TEXEL_WIDTH:1/e,CUBEUV_TEXEL_HEIGHT:1/t,CUBEUV_MAX_MIP:`${n}.0`},uniforms:{envMap:{value:null},sigma:{value:0},mipInt:{value:0}},vertexShader:wl(),fragmentShader:`

			precision highp float;
			precision highp int;

			varying vec3 vOutputDirection;

			uniform sampler2D envMap;
			uniform float sigma;
			uniform float mipInt;

			#define ENVMAP_TYPE_CUBE_UV
			#include <cube_uv_reflection_fragment>

			#define PI 3.14159265359
			#define GOLDEN_ANGLE 2.39996322973

			void main() {

				if ( sigma == 0.0 ) {

					gl_FragColor = vec4( bilinearCubeUV( envMap, vOutputDirection, mipInt ), 1.0 );
					return;

				}

				vec3 outputDirection = normalize( vOutputDirection );

				vec3 up = abs( outputDirection.z ) < 0.999 ? vec3( 0.0, 0.0, 1.0 ) : vec3( 1.0, 0.0, 0.0 );
				vec3 tangent = normalize( cross( up, outputDirection ) );
				vec3 bitangent = cross( outputDirection, tangent );

				// Truncate the kernel at three standard deviations or at the antipode.
				float thetaMax = min( 3.0 * sigma, PI );
				float truncation = 1.0 - exp( - 0.5 * thetaMax * thetaMax / ( sigma * sigma ) );

				vec3 accumColor = vec3( 0.0 );
				float accumWeight = 0.0;

				for ( int i = 0; i < SAMPLES; i ++ ) {

					// Stratified inverse-CDF sampling of the Gaussian, placed on a golden-angle spiral.
					float stratum = ( float( i ) + 0.5 ) / float( SAMPLES );
					float theta = sigma * sqrt( - 2.0 * log( 1.0 - stratum * truncation ) );
					float phi = float( i ) * GOLDEN_ANGLE;

					vec3 offset = cos( phi ) * tangent + sin( phi ) * bitangent;
					vec3 sampleDirection = cos( theta ) * outputDirection + sin( theta ) * offset;

					// Correct the planar sample density to solid angle.
					float weight = sin( theta ) / theta;

					accumColor += weight * bilinearCubeUV( envMap, sampleDirection, mipInt );
					accumWeight += weight;

				}

				gl_FragColor = vec4( accumColor / accumWeight, 1.0 );

			}
		`,blending:mi,depthTest:!1,depthWrite:!1})}function bd(){return new On({name:"EquirectangularToCubeUV",uniforms:{envMap:{value:null}},vertexShader:wl(),fragmentShader:`

			precision mediump float;
			precision mediump int;

			varying vec3 vOutputDirection;

			uniform sampler2D envMap;

			#include <common>

			void main() {

				vec3 outputDirection = normalize( vOutputDirection );
				vec2 uv = equirectUv( outputDirection );

				gl_FragColor = vec4( texture2D ( envMap, uv ).rgb, 1.0 );

			}
		`,blending:mi,depthTest:!1,depthWrite:!1})}function Md(){return new On({name:"CubemapToCubeUV",uniforms:{envMap:{value:null},flipEnvMap:{value:-1}},vertexShader:wl(),fragmentShader:`

			precision mediump float;
			precision mediump int;

			uniform float flipEnvMap;

			varying vec3 vOutputDirection;

			uniform samplerCube envMap;

			void main() {

				gl_FragColor = textureCube( envMap, vec3( flipEnvMap * vOutputDirection.x, vOutputDirection.yz ) );

			}
		`,blending:mi,depthTest:!1,depthWrite:!1})}function wl(){return`

		precision mediump float;
		precision mediump int;

		attribute vec3 outputDirection;

		varying vec3 vOutputDirection;

		void main() {

			vOutputDirection = outputDirection;
			gl_Position = vec4( position, 1.0 );

		}
	`}var Ml=class extends In{constructor(e=1,t={}){super(e,e,t),this.isWebGLCubeRenderTarget=!0;let i={width:e,height:e,depth:1},r=[i,i,i,i,i,i];this.texture=new zs(r),this._setTextureOptions(t),this.texture.isRenderTargetTexture=!0}fromEquirectangularTexture(e,t){this.texture.type=t.type,this.texture.colorSpace=t.colorSpace,this.texture.generateMipmaps=t.generateMipmaps,this.texture.minFilter=t.minFilter,this.texture.magFilter=t.magFilter;let i={uniforms:{tEquirect:{value:null}},vertexShader:`

				varying vec3 vWorldDirection;

				vec3 transformDirection( in vec3 dir, in mat4 matrix ) {

					return normalize( ( matrix * vec4( dir, 0.0 ) ).xyz );

				}

				void main() {

					vWorldDirection = transformDirection( position, modelMatrix );

					#include <begin_vertex>
					#include <project_vertex>

				}
			`,fragmentShader:`

				uniform sampler2D tEquirect;

				varying vec3 vWorldDirection;

				#include <common>

				void main() {

					vec3 direction = normalize( vWorldDirection );

					vec2 sampleUV = equirectUv( direction );

					gl_FragColor = texture2D( tEquirect, sampleUV );

				}
			`},r=new Ke(5,5,5),s=new On({name:"CubemapFromEquirect",uniforms:gr(i.uniforms),vertexShader:i.vertexShader,fragmentShader:i.fragmentShader,side:Rn,blending:mi});s.uniforms.tEquirect.value=t;let o=new xt(r,s),a=t.minFilter;return t.minFilter===Ji&&(t.minFilter=gn),new Ra(1,10,this).update(e,o),t.minFilter=a,o.geometry.dispose(),o.material.dispose(),this}clear(e,t=!0,i=!0,r=!0){let s=e.getRenderTarget();for(let o=0;o<6;o++)e.setRenderTarget(this,o),e.clear(t,i,r);e.setRenderTarget(s)}};function rx(n){let e=new WeakMap,t=new WeakMap,i=null;function r(u,g=!1){return u==null?null:g?o(u):s(u)}function s(u){if(u&&u.isTexture){let g=u.mapping;if(g===Ia||g===La)if(e.has(u)){let _=e.get(u).texture;return a(_,u.mapping)}else{let _=u.image;if(_&&_.height>0){let x=new Ml(_.height);return x.fromEquirectangularTexture(n,u),e.set(u,x),u.addEventListener("dispose",l),a(x.texture,u.mapping)}else return null}}return u}function o(u){if(u&&u.isTexture){let g=u.mapping,_=g===Ia||g===La,x=g===Ki||g===mr;if(_||x){let f=t.get(u),d=f!==void 0?f.texture.pmremVersion:0;if(u.isRenderTargetTexture&&u.pmremVersion!==d)return i===null&&(i=new bl(n)),f=_?i.fromEquirectangular(u,f):i.fromCubemap(u,f),f.texture.pmremVersion=u.pmremVersion,t.set(u,f),f.texture;if(f!==void 0)return f.texture;{let S=u.image;return _&&S&&S.height>0||x&&S&&c(S)?(i===null&&(i=new bl(n)),f=_?i.fromEquirectangular(u):i.fromCubemap(u),f.texture.pmremVersion=u.pmremVersion,t.set(u,f),u.addEventListener("dispose",h),f.texture):null}}}return u}function a(u,g){return g===Ia?u.mapping=Ki:g===La&&(u.mapping=mr),u}function c(u){let g=0,_=6;for(let x=0;x<_;x++)u[x]!==void 0&&g++;return g===_}function l(u){let g=u.target;g.removeEventListener("dispose",l);let _=e.get(g);_!==void 0&&(e.delete(g),_.dispose())}function h(u){let g=u.target;g.removeEventListener("dispose",h);let _=t.get(g);_!==void 0&&(t.delete(g),_.dispose())}function m(){e=new WeakMap,t=new WeakMap,i!==null&&(i.dispose(),i=null)}return{get:r,dispose:m}}function sx(n){let e={};function t(i){if(e[i]!==void 0)return e[i];let r=n.getExtension(i);return e[i]=r,r}return{has:function(i){return t(i)!==null},init:function(){t("EXT_color_buffer_float"),t("WEBGL_clip_cull_distance"),t("OES_texture_float_linear"),t("EXT_color_buffer_half_float"),t("WEBGL_multisampled_render_to_texture"),t("WEBGL_render_shared_exponent")},get:function(i){let r=t(i);return r===null&&cr("WebGLRenderer: "+i+" extension not supported."),r}}}function ox(n,e,t,i){let r={},s=new WeakMap;function o(m){let u=m.target;u.index!==null&&e.remove(u.index);for(let _ in u.attributes)e.remove(u.attributes[_]);u.removeEventListener("dispose",o),delete r[u.id];let g=s.get(u);g&&(e.remove(g),s.delete(u)),i.releaseStatesOfGeometry(u),u.isInstancedBufferGeometry===!0&&delete u._maxInstanceCount,t.memory.geometries--}function a(m,u){return r[u.id]===!0||(u.addEventListener("dispose",o),r[u.id]=!0,t.memory.geometries++),u}function c(m){let u=m.attributes;for(let g in u)e.update(u[g],n.ARRAY_BUFFER)}function l(m){let u=[],g=m.index,_=m.attributes.position,x=0;if(_===void 0)return;if(g!==null){let S=g.array;x=g.version;for(let P=0,w=S.length;P<w;P+=3){let T=S[P+0],A=S[P+1],D=S[P+2];u.push(T,A,A,D,D,T)}}else{let S=_.array;x=_.version;for(let P=0,w=S.length/3-1;P<w;P+=3){let T=P+0,A=P+1,D=P+2;u.push(T,A,A,D,D,T)}}let f=new(_.count>=65535?Os:Fs)(u,1);f.version=x;let d=s.get(m);d&&e.remove(d),s.set(m,f)}function h(m){let u=s.get(m);if(u){let g=m.index;g!==null&&u.version<g.version&&l(m)}else l(m);return s.get(m)}return{get:a,update:c,getWireframeAttribute:h}}function ax(n,e,t){let i;function r(m){i=m}let s,o;function a(m){s=m.type,o=m.bytesPerElement}function c(m,u){n.drawElements(i,u,s,m*o),t.update(u,i,1)}function l(m,u,g){g!==0&&(n.drawElementsInstanced(i,u,s,m*o,g),t.update(u,i,g))}function h(m,u,g){if(g===0)return;e.get("WEBGL_multi_draw").multiDrawElementsWEBGL(i,u,0,s,m,0,g);let x=0;for(let f=0;f<g;f++)x+=u[f];t.update(x,i,1)}this.setMode=r,this.setIndex=a,this.render=c,this.renderInstances=l,this.renderMultiDraw=h}function lx(n){let e={geometries:0,textures:0},t={frame:0,calls:0,triangles:0,points:0,lines:0};function i(s,o,a){switch(t.calls++,o){case n.TRIANGLES:t.triangles+=a*(s/3);break;case n.LINES:t.lines+=a*(s/2);break;case n.LINE_STRIP:t.lines+=a*(s-1);break;case n.LINE_LOOP:t.lines+=a*s;break;case n.POINTS:t.points+=a*s;break;default:Xe("WebGLInfo: Unknown draw mode:",o);break}}function r(){t.calls=0,t.triangles=0,t.points=0,t.lines=0}return{memory:e,render:t,programs:null,autoReset:!0,reset:r,update:i}}function cx(n,e,t){let i=new WeakMap,r=new Ot;function s(o,a,c){let l=o.morphTargetInfluences,h=a.morphAttributes.position||a.morphAttributes.normal||a.morphAttributes.color,m=h!==void 0?h.length:0,u=i.get(a);if(u===void 0||u.count!==m){let M=function(){D.dispose(),i.delete(a),a.removeEventListener("dispose",M)};u!==void 0&&u.texture.dispose();let g=a.morphAttributes.position!==void 0,_=a.morphAttributes.normal!==void 0,x=a.morphAttributes.color!==void 0,f=a.morphAttributes.position||[],d=a.morphAttributes.normal||[],S=a.morphAttributes.color||[],P=0;g===!0&&(P=1),_===!0&&(P=2),x===!0&&(P=3);let w=a.attributes.position.count*P,T=1;w>e.maxTextureSize&&(T=Math.ceil(w/e.maxTextureSize),w=e.maxTextureSize);let A=new Float32Array(w*T*4*m),D=new Ns(A,w,T,m);D.type=ri,D.needsUpdate=!0;let p=P*4;for(let y=0;y<m;y++){let C=f[y],k=d[y],L=S[y],U=w*T*4*y;for(let F=0;F<C.count;F++){let H=F*p;g===!0&&(r.fromBufferAttribute(C,F),A[U+H+0]=r.x,A[U+H+1]=r.y,A[U+H+2]=r.z,A[U+H+3]=0),_===!0&&(r.fromBufferAttribute(k,F),A[U+H+4]=r.x,A[U+H+5]=r.y,A[U+H+6]=r.z,A[U+H+7]=0),x===!0&&(r.fromBufferAttribute(L,F),A[U+H+8]=r.x,A[U+H+9]=r.y,A[U+H+10]=r.z,A[U+H+11]=L.itemSize===4?r.w:1)}}u={count:m,texture:D,size:new He(w,T)},i.set(a,u),a.addEventListener("dispose",M)}if(o.isInstancedMesh===!0&&o.morphTexture!==null)c.getUniforms().setValue(n,"morphTexture",o.morphTexture,t);else{let g=0;for(let x=0;x<l.length;x++)g+=l[x];let _=a.morphTargetsRelative?1:1-g;c.getUniforms().setValue(n,"morphTargetBaseInfluence",_),c.getUniforms().setValue(n,"morphTargetInfluences",l)}c.getUniforms().setValue(n,"morphTargetsTexture",u.texture,t),c.getUniforms().setValue(n,"morphTargetsTextureSize",u.size)}return{update:s}}function ux(n,e,t,i,r){let s=new WeakMap;function o(l){let h=r.render.frame,m=l.geometry,u=e.get(l,m);if(s.get(u)!==h&&(e.update(u),s.set(u,h)),l.isInstancedMesh&&(l.hasEventListener("dispose",c)===!1&&l.addEventListener("dispose",c),s.get(l)!==h&&(t.update(l.instanceMatrix,n.ARRAY_BUFFER),l.instanceColor!==null&&t.update(l.instanceColor,n.ARRAY_BUFFER),s.set(l,h))),l.isSkinnedMesh){let g=l.skeleton;s.get(g)!==h&&(g.update(),s.set(g,h))}return u}function a(){s=new WeakMap}function c(l){let h=l.target;h.removeEventListener("dispose",c),i.releaseStatesOfObject(h),t.remove(h.instanceMatrix),h.instanceColor!==null&&t.remove(h.instanceColor)}return{update:o,dispose:a}}var hx={[kc]:"LINEAR_TONE_MAPPING",[Dc]:"REINHARD_TONE_MAPPING",[Nc]:"CINEON_TONE_MAPPING",[Uc]:"ACES_FILMIC_TONE_MAPPING",[Oc]:"AGX_TONE_MAPPING",[Bc]:"NEUTRAL_TONE_MAPPING",[Fc]:"CUSTOM_TONE_MAPPING"};function dx(n,e,t,i,r,s){let o=new In(e,t,{type:n,depthBuffer:r,stencilBuffer:s,samples:i?4:0,storeMultisampledDepthBuffer:!1,storeMultisampledStencilBuffer:!1,resolveDepthBuffer:!1,resolveStencilBuffer:!1}),a=null,c=null,l=new Xt;l.setAttribute("position",new ct([-1,3,0,-1,-1,0,3,-1,0],3)),l.setAttribute("uv",new ct([0,2,0,0,2,0],2));let h=new ma({uniforms:{tDiffuse:{value:null}},vertexShader:`
			precision highp float;

			uniform mat4 modelViewMatrix;
			uniform mat4 projectionMatrix;

			attribute vec3 position;
			attribute vec2 uv;

			varying vec2 vUv;

			void main() {
				vUv = uv;
				gl_Position = projectionMatrix * modelViewMatrix * vec4( position, 1.0 );
			}`,fragmentShader:`
			precision highp float;

			uniform sampler2D tDiffuse;

			varying vec2 vUv;

			#include <tonemapping_pars_fragment>
			#include <colorspace_pars_fragment>

			void main() {
				gl_FragColor = texture2D( tDiffuse, vUv );

				#ifdef LINEAR_TONE_MAPPING
					gl_FragColor.rgb = LinearToneMapping( gl_FragColor.rgb );
				#elif defined( REINHARD_TONE_MAPPING )
					gl_FragColor.rgb = ReinhardToneMapping( gl_FragColor.rgb );
				#elif defined( CINEON_TONE_MAPPING )
					gl_FragColor.rgb = CineonToneMapping( gl_FragColor.rgb );
				#elif defined( ACES_FILMIC_TONE_MAPPING )
					gl_FragColor.rgb = ACESFilmicToneMapping( gl_FragColor.rgb );
				#elif defined( AGX_TONE_MAPPING )
					gl_FragColor.rgb = AgXToneMapping( gl_FragColor.rgb );
				#elif defined( NEUTRAL_TONE_MAPPING )
					gl_FragColor.rgb = NeutralToneMapping( gl_FragColor.rgb );
				#elif defined( CUSTOM_TONE_MAPPING )
					gl_FragColor.rgb = CustomToneMapping( gl_FragColor.rgb );
				#endif

				#ifdef SRGB_TRANSFER
					gl_FragColor = sRGBTransferOETF( gl_FragColor );
				#endif
			}`,depthTest:!1,depthWrite:!1}),m=new xt(l,h),u=new rs(-1,1,1,-1,0,1),g=null,_=null,x=!1,f,d=null,S=[],P=!1;this.setSize=function(w,T){o.setSize(w,T),a!==null&&a.setSize(w,T),c!==null&&c.setSize(w,T);for(let A=0;A<S.length;A++){let D=S[A];D.setSize&&D.setSize(w,T)}},this.setEffects=function(w){S=w,P=S.length>0&&S[0].isRenderPass===!0;let T=o.width,A=o.height;S.length>0&&a===null&&(a=new In(T,A,{type:si,depthBuffer:!1,stencilBuffer:!1}),c=new In(T,A,{type:si,depthBuffer:!1,stencilBuffer:!1}));for(let D=0;D<S.length;D++){let p=S[D];p.setSize&&p.setSize(T,A)}},this.begin=function(w,T){if(x||w.toneMapping===ni&&S.length===0)return!1;if(d=T,T!==null){let A=T.width,D=T.height;(o.width!==A||o.height!==D)&&this.setSize(A,D)}return P===!1&&w.setRenderTarget(o),f=w.toneMapping,w.toneMapping=ni,!0},this.hasRenderPass=function(){return P},this.end=function(w,T){w.toneMapping=f,x=!0;let A=o,D=a;for(let p=0;p<S.length;p++){let M=S[p];M.enabled!==!1&&(M.render(w,D,A,T),M.needsSwap!==!1&&(A=D,D=D===a?c:a))}if(g!==w.outputColorSpace||_!==w.toneMapping){g=w.outputColorSpace,_=w.toneMapping,h.defines={},lt.getTransfer(g)===Mt&&(h.defines.SRGB_TRANSFER="");let p=hx[_];p&&(h.defines[p]=""),h.needsUpdate=!0}h.uniforms.tDiffuse.value=A.texture,w.setRenderTarget(d),w.render(m,u),d=null,x=!1},this.isCompositing=function(){return x},this.dispose=function(){o.dispose(),a!==null&&a.dispose(),c!==null&&c.dispose(),l.dispose(),h.dispose()}}var Vd=new An,lu=new Vi(1,1),Gd=new Ns,Wd=new la,Xd=new zs,Sd=[],wd=[],Ed=new Float32Array(16),Td=new Float32Array(9),Ad=new Float32Array(4);function ms(n,e,t){let i=n[0];if(i<=0||i>0)return n;let r=e*t,s=Sd[r];if(s===void 0&&(s=new Float32Array(r),Sd[r]=s),e!==0){i.toArray(s,0);for(let o=1,a=0;o!==e;++o)a+=t,n[o].toArray(s,a)}return s}function an(n,e){if(n.length!==e.length)return!1;for(let t=0,i=n.length;t<i;t++)if(n[t]!==e[t])return!1;return!0}function ln(n,e){for(let t=0,i=e.length;t<i;t++)n[t]=e[t]}function El(n,e){let t=wd[e];t===void 0&&(t=new Int32Array(e),wd[e]=t);for(let i=0;i!==e;++i)t[i]=n.allocateTextureUnit();return t}function fx(n,e){let t=this.cache;t[0]!==e&&(n.uniform1f(this.addr,e),t[0]=e)}function px(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y)&&(n.uniform2f(this.addr,e.x,e.y),t[0]=e.x,t[1]=e.y);else{if(an(t,e))return;n.uniform2fv(this.addr,e),ln(t,e)}}function mx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z)&&(n.uniform3f(this.addr,e.x,e.y,e.z),t[0]=e.x,t[1]=e.y,t[2]=e.z);else if(e.r!==void 0)(t[0]!==e.r||t[1]!==e.g||t[2]!==e.b)&&(n.uniform3f(this.addr,e.r,e.g,e.b),t[0]=e.r,t[1]=e.g,t[2]=e.b);else{if(an(t,e))return;n.uniform3fv(this.addr,e),ln(t,e)}}function gx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z||t[3]!==e.w)&&(n.uniform4f(this.addr,e.x,e.y,e.z,e.w),t[0]=e.x,t[1]=e.y,t[2]=e.z,t[3]=e.w);else{if(an(t,e))return;n.uniform4fv(this.addr,e),ln(t,e)}}function xx(n,e){let t=this.cache,i=e.elements;if(i===void 0){if(an(t,e))return;n.uniformMatrix2fv(this.addr,!1,e),ln(t,e)}else{if(an(t,i))return;Ad.set(i),n.uniformMatrix2fv(this.addr,!1,Ad),ln(t,i)}}function vx(n,e){let t=this.cache,i=e.elements;if(i===void 0){if(an(t,e))return;n.uniformMatrix3fv(this.addr,!1,e),ln(t,e)}else{if(an(t,i))return;Td.set(i),n.uniformMatrix3fv(this.addr,!1,Td),ln(t,i)}}function _x(n,e){let t=this.cache,i=e.elements;if(i===void 0){if(an(t,e))return;n.uniformMatrix4fv(this.addr,!1,e),ln(t,e)}else{if(an(t,i))return;Ed.set(i),n.uniformMatrix4fv(this.addr,!1,Ed),ln(t,i)}}function yx(n,e){let t=this.cache;t[0]!==e&&(n.uniform1i(this.addr,e),t[0]=e)}function bx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y)&&(n.uniform2i(this.addr,e.x,e.y),t[0]=e.x,t[1]=e.y);else{if(an(t,e))return;n.uniform2iv(this.addr,e),ln(t,e)}}function Mx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z)&&(n.uniform3i(this.addr,e.x,e.y,e.z),t[0]=e.x,t[1]=e.y,t[2]=e.z);else{if(an(t,e))return;n.uniform3iv(this.addr,e),ln(t,e)}}function Sx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z||t[3]!==e.w)&&(n.uniform4i(this.addr,e.x,e.y,e.z,e.w),t[0]=e.x,t[1]=e.y,t[2]=e.z,t[3]=e.w);else{if(an(t,e))return;n.uniform4iv(this.addr,e),ln(t,e)}}function wx(n,e){let t=this.cache;t[0]!==e&&(n.uniform1ui(this.addr,e),t[0]=e)}function Ex(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y)&&(n.uniform2ui(this.addr,e.x,e.y),t[0]=e.x,t[1]=e.y);else{if(an(t,e))return;n.uniform2uiv(this.addr,e),ln(t,e)}}function Tx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z)&&(n.uniform3ui(this.addr,e.x,e.y,e.z),t[0]=e.x,t[1]=e.y,t[2]=e.z);else{if(an(t,e))return;n.uniform3uiv(this.addr,e),ln(t,e)}}function Ax(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z||t[3]!==e.w)&&(n.uniform4ui(this.addr,e.x,e.y,e.z,e.w),t[0]=e.x,t[1]=e.y,t[2]=e.z,t[3]=e.w);else{if(an(t,e))return;n.uniform4uiv(this.addr,e),ln(t,e)}}function Rx(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r);let s;this.type===n.SAMPLER_2D_SHADOW?(lu.compareFunction=t.isReversedDepthBuffer()?vl:xl,s=lu):s=Vd,t.setTexture2D(e||s,r)}function Cx(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r),t.setTexture3D(e||Wd,r)}function Px(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r),t.setTextureCube(e||Xd,r)}function Ix(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r),t.setTexture2DArray(e||Gd,r)}function Lx(n){switch(n){case 5126:return fx;case 35664:return px;case 35665:return mx;case 35666:return gx;case 35674:return xx;case 35675:return vx;case 35676:return _x;case 5124:case 35670:return yx;case 35667:case 35671:return bx;case 35668:case 35672:return Mx;case 35669:case 35673:return Sx;case 5125:return wx;case 36294:return Ex;case 36295:return Tx;case 36296:return Ax;case 35678:case 36198:case 36298:case 36306:case 35682:return Rx;case 35679:case 36299:case 36307:return Cx;case 35680:case 36300:case 36308:case 36293:return Px;case 36289:case 36303:case 36311:case 36292:return Ix}}function kx(n,e){n.uniform1fv(this.addr,e)}function Dx(n,e){let t=ms(e,this.size,2);n.uniform2fv(this.addr,t)}function Nx(n,e){let t=ms(e,this.size,3);n.uniform3fv(this.addr,t)}function Ux(n,e){let t=ms(e,this.size,4);n.uniform4fv(this.addr,t)}function Fx(n,e){let t=ms(e,this.size,4);n.uniformMatrix2fv(this.addr,!1,t)}function Ox(n,e){let t=ms(e,this.size,9);n.uniformMatrix3fv(this.addr,!1,t)}function Bx(n,e){let t=ms(e,this.size,16);n.uniformMatrix4fv(this.addr,!1,t)}function zx(n,e){n.uniform1iv(this.addr,e)}function Hx(n,e){n.uniform2iv(this.addr,e)}function Vx(n,e){n.uniform3iv(this.addr,e)}function Gx(n,e){n.uniform4iv(this.addr,e)}function Wx(n,e){n.uniform1uiv(this.addr,e)}function Xx(n,e){n.uniform2uiv(this.addr,e)}function qx(n,e){n.uniform3uiv(this.addr,e)}function $x(n,e){n.uniform4uiv(this.addr,e)}function Yx(n,e,t){let i=this.cache,r=e.length,s=El(t,r);an(i,s)||(n.uniform1iv(this.addr,s),ln(i,s));let o;this.type===n.SAMPLER_2D_SHADOW?o=lu:o=Vd;for(let a=0;a!==r;++a)t.setTexture2D(e[a]||o,s[a])}function Zx(n,e,t){let i=this.cache,r=e.length,s=El(t,r);an(i,s)||(n.uniform1iv(this.addr,s),ln(i,s));for(let o=0;o!==r;++o)t.setTexture3D(e[o]||Wd,s[o])}function Kx(n,e,t){let i=this.cache,r=e.length,s=El(t,r);an(i,s)||(n.uniform1iv(this.addr,s),ln(i,s));for(let o=0;o!==r;++o)t.setTextureCube(e[o]||Xd,s[o])}function Jx(n,e,t){let i=this.cache,r=e.length,s=El(t,r);an(i,s)||(n.uniform1iv(this.addr,s),ln(i,s));for(let o=0;o!==r;++o)t.setTexture2DArray(e[o]||Gd,s[o])}function jx(n){switch(n){case 5126:return kx;case 35664:return Dx;case 35665:return Nx;case 35666:return Ux;case 35674:return Fx;case 35675:return Ox;case 35676:return Bx;case 5124:case 35670:return zx;case 35667:case 35671:return Hx;case 35668:case 35672:return Vx;case 35669:case 35673:return Gx;case 5125:return Wx;case 36294:return Xx;case 36295:return qx;case 36296:return $x;case 35678:case 36198:case 36298:case 36306:case 35682:return Yx;case 35679:case 36299:case 36307:return Zx;case 35680:case 36300:case 36308:case 36293:return Kx;case 36289:case 36303:case 36311:case 36292:return Jx}}var cu=class{constructor(e,t,i){this.id=e,this.addr=i,this.cache=[],this.type=t.type,this.setValue=Lx(t.type)}},uu=class{constructor(e,t,i){this.id=e,this.addr=i,this.cache=[],this.type=t.type,this.size=t.size,this.setValue=jx(t.type)}},hu=class{constructor(e){this.id=e,this.seq=[],this.map={}}setValue(e,t,i){let r=this.seq;for(let s=0,o=r.length;s!==o;++s){let a=r[s];a.setValue(e,t[a.id],i)}}},ou=/(\w+)(\])?(\[|\.)?/g;function Rd(n,e){n.seq.push(e),n.map[e.id]=e}function Qx(n,e,t){let i=n.name,r=i.length;for(ou.lastIndex=0;;){let s=ou.exec(i),o=ou.lastIndex,a=s[1],c=s[2]==="]",l=s[3];if(c&&(a=a|0),l===void 0||l==="["&&o+2===r){Rd(t,l===void 0?new cu(a,n,e):new uu(a,n,e));break}else{let m=t.map[a];m===void 0&&(m=new hu(a),Rd(t,m)),t=m}}}var ps=class{constructor(e,t){this.seq=[],this.map={};let i=e.getProgramParameter(t,e.ACTIVE_UNIFORMS);for(let o=0;o<i;++o){let a=e.getActiveUniform(t,o),c=e.getUniformLocation(t,a.name);Qx(a,c,this)}let r=[],s=[];for(let o of this.seq)o.type===e.SAMPLER_2D_SHADOW||o.type===e.SAMPLER_CUBE_SHADOW||o.type===e.SAMPLER_2D_ARRAY_SHADOW?r.push(o):s.push(o);r.length>0&&(this.seq=r.concat(s))}setValue(e,t,i,r){let s=this.map[t];s!==void 0&&s.setValue(e,i,r)}setOptional(e,t,i){let r=t[i];r!==void 0&&this.setValue(e,i,r)}static upload(e,t,i,r){for(let s=0,o=t.length;s!==o;++s){let a=t[s],c=i[a.id];c.needsUpdate!==!1&&a.setValue(e,c.value,r)}}static seqWithValue(e,t){let i=[];for(let r=0,s=e.length;r!==s;++r){let o=e[r];o.id in t&&i.push(o)}return i}};function Cd(n,e,t){let i=n.createShader(e);return n.shaderSource(i,t),n.compileShader(i),i}var ev=37297,tv=0;function nv(n,e){let t=n.split(`
`),i=[],r=Math.max(e-6,0),s=Math.min(e+6,t.length);for(let o=r;o<s;o++){let a=o+1;i.push(`${a===e?">":" "} ${a}: ${t[o]}`)}return i.join(`
`)}var Pd=new Ze;function iv(n){lt._getMatrix(Pd,lt.workingColorSpace,n);let e=`mat3( ${Pd.elements.map(t=>t.toFixed(4))} )`;switch(lt.getTransfer(n)){case ks:return[e,"LinearTransferOETF"];case Mt:return[e,"sRGBTransferOETF"];default:return Ge("WebGLProgram: Unsupported color space: ",n),[e,"LinearTransferOETF"]}}function Id(n,e,t){let i=n.getShaderParameter(e,n.COMPILE_STATUS),s=(n.getShaderInfoLog(e)||"").trim();if(i&&s==="")return"";let o=/ERROR: 0:(\d+)/.exec(s);if(o){let a=parseInt(o[1]);return t.toUpperCase()+`

`+s+`

`+nv(n.getShaderSource(e),a)}else return s}function rv(n,e){let t=iv(e);return[`vec4 ${n}( vec4 value ) {`,`	return ${t[1]}( vec4( value.rgb * ${t[0]}, value.a ) );`,"}"].join(`
`)}var sv={[kc]:"Linear",[Dc]:"Reinhard",[Nc]:"Cineon",[Uc]:"ACESFilmic",[Oc]:"AgX",[Bc]:"Neutral",[Fc]:"Custom"};function ov(n,e){let t=sv[e];return t===void 0?(Ge("WebGLProgram: Unsupported toneMapping:",e),"vec3 "+n+"( vec3 color ) { return LinearToneMapping( color ); }"):"vec3 "+n+"( vec3 color ) { return "+t+"ToneMapping( color ); }"}var yl=new V;function av(){lt.getLuminanceCoefficients(yl);let n=yl.x.toFixed(4),e=yl.y.toFixed(4),t=yl.z.toFixed(4);return["float luminance( const in vec3 rgb ) {",`	const vec3 weights = vec3( ${n}, ${e}, ${t} );`,"	return dot( weights, rgb );","}"].join(`
`)}function lv(n){return[n.extensionClipCullDistance?"#extension GL_ANGLE_clip_cull_distance : require":"",n.extensionMultiDraw?"#extension GL_ANGLE_multi_draw : require":""].filter(ho).join(`
`)}function cv(n){let e=[];for(let t in n){let i=n[t];i!==!1&&e.push("#define "+t+" "+i)}return e.join(`
`)}function uv(n,e){let t={},i=n.getProgramParameter(e,n.ACTIVE_ATTRIBUTES);for(let r=0;r<i;r++){let s=n.getActiveAttrib(e,r),o=s.name,a=1;s.type===n.FLOAT_MAT2&&(a=2),s.type===n.FLOAT_MAT3&&(a=3),s.type===n.FLOAT_MAT4&&(a=4),t[o]={type:s.type,location:n.getAttribLocation(e,o),locationSize:a}}return t}function ho(n){return n!==""}function Ld(n,e){let t=e.numSpotLightShadows+e.numSpotLightMaps-e.numSpotLightShadowsWithMaps;return n.replace(/NUM_SUN_LIGHTS/g,e.numSunLights).replace(/NUM_DIR_LIGHTS/g,e.numDirLights).replace(/NUM_SPOT_LIGHTS/g,e.numSpotLights).replace(/NUM_SPOT_LIGHT_MAPS/g,e.numSpotLightMaps).replace(/NUM_SPOT_LIGHT_COORDS/g,t).replace(/NUM_RECT_AREA_LIGHTS/g,e.numRectAreaLights).replace(/NUM_POINT_LIGHTS/g,e.numPointLights).replace(/NUM_HEMI_LIGHTS/g,e.numHemiLights).replace(/NUM_SUN_LIGHT_SHADOWS/g,e.numSunLightShadows).replace(/NUM_DIR_LIGHT_SHADOWS/g,e.numDirLightShadows).replace(/NUM_SPOT_LIGHT_SHADOWS_WITH_MAPS/g,e.numSpotLightShadowsWithMaps).replace(/NUM_SPOT_LIGHT_SHADOWS/g,e.numSpotLightShadows).replace(/NUM_POINT_LIGHT_SHADOWS/g,e.numPointLightShadows)}function kd(n,e){return n.replace(/NUM_CLIPPING_PLANES/g,e.numClippingPlanes).replace(/UNION_CLIPPING_PLANES/g,e.numClippingPlanes-e.numClipIntersection)}var hv=/^[ \t]*#include +<([\w\d./]+)>/gm;function du(n){return n.replace(hv,fv)}var dv=new Map;function fv(n,e){let t=tt[e];if(t===void 0){let i=dv.get(e);if(i!==void 0)t=tt[i],Ge('WebGLRenderer: Shader chunk "%s" has been deprecated. Use "%s" instead.',e,i);else throw new Error("THREE.WebGLProgram: Can not resolve #include <"+e+">")}return du(t)}var pv=/#pragma unroll_loop_start\s+for\s*\(\s*int\s+i\s*=\s*(\d+)\s*;\s*i\s*<\s*(\d+)\s*;\s*i\s*\+\+\s*\)\s*{([\s\S]+?)}\s+#pragma unroll_loop_end/g;function Dd(n){return n.replace(pv,mv)}function mv(n,e,t,i){let r="";for(let s=parseInt(e);s<parseInt(t);s++)r+=i.replace(/\[\s*i\s*\]/g,"[ "+s+" ]").replace(/UNROLLED_LOOP_INDEX/g,s);return r}function Nd(n){let e=`precision ${n.precision} float;
	precision ${n.precision} int;
	precision ${n.precision} sampler2D;
	precision ${n.precision} samplerCube;
	precision ${n.precision} sampler3D;
	precision ${n.precision} sampler2DArray;
	precision ${n.precision} sampler2DShadow;
	precision ${n.precision} samplerCubeShadow;
	precision ${n.precision} sampler2DArrayShadow;
	precision ${n.precision} isampler2D;
	precision ${n.precision} isampler3D;
	precision ${n.precision} isamplerCube;
	precision ${n.precision} isampler2DArray;
	precision ${n.precision} usampler2D;
	precision ${n.precision} usampler3D;
	precision ${n.precision} usamplerCube;
	precision ${n.precision} usampler2DArray;
	`;return n.precision==="highp"?e+=`
#define HIGH_PRECISION`:n.precision==="mediump"?e+=`
#define MEDIUM_PRECISION`:n.precision==="lowp"&&(e+=`
#define LOW_PRECISION`),e}var gv={[eo]:"SHADOWMAP_TYPE_PCF",[os]:"SHADOWMAP_TYPE_VSM"};function xv(n){return gv[n.shadowMapType]||"SHADOWMAP_TYPE_BASIC"}var vv={[Ki]:"ENVMAP_TYPE_CUBE",[mr]:"ENVMAP_TYPE_CUBE",[to]:"ENVMAP_TYPE_CUBE_UV"};function _v(n){return n.envMap===!1?"ENVMAP_TYPE_CUBE":vv[n.envMapMode]||"ENVMAP_TYPE_CUBE"}var yv={[mr]:"ENVMAP_MODE_REFRACTION"};function bv(n){return n.envMap===!1?"ENVMAP_MODE_REFLECTION":yv[n.envMapMode]||"ENVMAP_MODE_REFLECTION"}var Mv={[Lc]:"ENVMAP_BLENDING_MULTIPLY",[jh]:"ENVMAP_BLENDING_MIX",[Qh]:"ENVMAP_BLENDING_ADD"};function Sv(n){return n.envMap===!1?"ENVMAP_BLENDING_NONE":Mv[n.combine]||"ENVMAP_BLENDING_NONE"}function wv(n){let e=n.envMapCubeUVHeight;if(e===null)return null;let t=Math.log2(e)-2,i=1/e;return{texelWidth:1/(3*Math.max(Math.pow(2,t),112)),texelHeight:i,maxMip:t}}function Ev(n,e,t,i){let r=n.getContext(),s=t.defines,o=t.vertexShader,a=t.fragmentShader,c=xv(t),l=_v(t),h=bv(t),m=Sv(t),u=wv(t),g=lv(t),_=cv(s),x=r.createProgram(),f,d,S=t.glslVersion?"#version "+t.glslVersion+`
`:"";t.isRawShaderMaterial?(f=["#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,_].filter(ho).join(`
`),f.length>0&&(f+=`
`),d=["#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,_].filter(ho).join(`
`),d.length>0&&(d+=`
`)):(f=[Nd(t),"#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,_,t.extensionClipCullDistance?"#define USE_CLIP_DISTANCE":"",t.batching?"#define USE_BATCHING":"",t.batchingColor?"#define USE_BATCHING_COLOR":"",t.instancing?"#define USE_INSTANCING":"",t.instancingColor?"#define USE_INSTANCING_COLOR":"",t.instancingMorph?"#define USE_INSTANCING_MORPH":"",t.useFog&&t.fog?"#define USE_FOG":"",t.useFog&&t.fogExp2?"#define FOG_EXP2":"",t.map?"#define USE_MAP":"",t.envMap?"#define USE_ENVMAP":"",t.envMap?"#define "+h:"",t.lightMap?"#define USE_LIGHTMAP":"",t.aoMap?"#define USE_AOMAP":"",t.bumpMap?"#define USE_BUMPMAP":"",t.normalMap?"#define USE_NORMALMAP":"",t.normalMapObjectSpace?"#define USE_NORMALMAP_OBJECTSPACE":"",t.normalMapTangentSpace?"#define USE_NORMALMAP_TANGENTSPACE":"",t.displacementMap?"#define USE_DISPLACEMENTMAP":"",t.emissiveMap?"#define USE_EMISSIVEMAP":"",t.anisotropy?"#define USE_ANISOTROPY":"",t.anisotropyMap?"#define USE_ANISOTROPYMAP":"",t.clearcoatMap?"#define USE_CLEARCOATMAP":"",t.clearcoatRoughnessMap?"#define USE_CLEARCOAT_ROUGHNESSMAP":"",t.clearcoatNormalMap?"#define USE_CLEARCOAT_NORMALMAP":"",t.iridescenceMap?"#define USE_IRIDESCENCEMAP":"",t.iridescenceThicknessMap?"#define USE_IRIDESCENCE_THICKNESSMAP":"",t.specularMap?"#define USE_SPECULARMAP":"",t.specularColorMap?"#define USE_SPECULAR_COLORMAP":"",t.specularIntensityMap?"#define USE_SPECULAR_INTENSITYMAP":"",t.roughnessMap?"#define USE_ROUGHNESSMAP":"",t.metalnessMap?"#define USE_METALNESSMAP":"",t.alphaMap?"#define USE_ALPHAMAP":"",t.alphaHash?"#define USE_ALPHAHASH":"",t.transmission?"#define USE_TRANSMISSION":"",t.transmissionMap?"#define USE_TRANSMISSIONMAP":"",t.thicknessMap?"#define USE_THICKNESSMAP":"",t.sheenColorMap?"#define USE_SHEEN_COLORMAP":"",t.sheenRoughnessMap?"#define USE_SHEEN_ROUGHNESSMAP":"",t.mapUv?"#define MAP_UV "+t.mapUv:"",t.alphaMapUv?"#define ALPHAMAP_UV "+t.alphaMapUv:"",t.lightMapUv?"#define LIGHTMAP_UV "+t.lightMapUv:"",t.aoMapUv?"#define AOMAP_UV "+t.aoMapUv:"",t.emissiveMapUv?"#define EMISSIVEMAP_UV "+t.emissiveMapUv:"",t.bumpMapUv?"#define BUMPMAP_UV "+t.bumpMapUv:"",t.normalMapUv?"#define NORMALMAP_UV "+t.normalMapUv:"",t.displacementMapUv?"#define DISPLACEMENTMAP_UV "+t.displacementMapUv:"",t.metalnessMapUv?"#define METALNESSMAP_UV "+t.metalnessMapUv:"",t.roughnessMapUv?"#define ROUGHNESSMAP_UV "+t.roughnessMapUv:"",t.anisotropyMapUv?"#define ANISOTROPYMAP_UV "+t.anisotropyMapUv:"",t.clearcoatMapUv?"#define CLEARCOATMAP_UV "+t.clearcoatMapUv:"",t.clearcoatNormalMapUv?"#define CLEARCOAT_NORMALMAP_UV "+t.clearcoatNormalMapUv:"",t.clearcoatRoughnessMapUv?"#define CLEARCOAT_ROUGHNESSMAP_UV "+t.clearcoatRoughnessMapUv:"",t.iridescenceMapUv?"#define IRIDESCENCEMAP_UV "+t.iridescenceMapUv:"",t.iridescenceThicknessMapUv?"#define IRIDESCENCE_THICKNESSMAP_UV "+t.iridescenceThicknessMapUv:"",t.sheenColorMapUv?"#define SHEEN_COLORMAP_UV "+t.sheenColorMapUv:"",t.sheenRoughnessMapUv?"#define SHEEN_ROUGHNESSMAP_UV "+t.sheenRoughnessMapUv:"",t.specularMapUv?"#define SPECULARMAP_UV "+t.specularMapUv:"",t.specularColorMapUv?"#define SPECULAR_COLORMAP_UV "+t.specularColorMapUv:"",t.specularIntensityMapUv?"#define SPECULAR_INTENSITYMAP_UV "+t.specularIntensityMapUv:"",t.transmissionMapUv?"#define TRANSMISSIONMAP_UV "+t.transmissionMapUv:"",t.thicknessMapUv?"#define THICKNESSMAP_UV "+t.thicknessMapUv:"",t.vertexTangents&&t.flatShading===!1?"#define USE_TANGENT":"",t.vertexNormals?"#define HAS_NORMAL":"",t.vertexColors?"#define USE_COLOR":"",t.vertexAlphas?"#define USE_COLOR_ALPHA":"",t.vertexUv1s?"#define USE_UV1":"",t.vertexUv2s?"#define USE_UV2":"",t.vertexUv3s?"#define USE_UV3":"",t.pointsUvs?"#define USE_POINTS_UV":"",t.flatShading?"#define FLAT_SHADED":"",t.skinning?"#define USE_SKINNING":"",t.morphTargets?"#define USE_MORPHTARGETS":"",t.morphNormals&&t.flatShading===!1?"#define USE_MORPHNORMALS":"",t.morphColors?"#define USE_MORPHCOLORS":"",t.morphTargetsCount>0?"#define MORPHTARGETS_TEXTURE_STRIDE "+t.morphTextureStride:"",t.morphTargetsCount>0?"#define MORPHTARGETS_COUNT "+t.morphTargetsCount:"",t.doubleSided?"#define DOUBLE_SIDED":"",t.flipSided?"#define FLIP_SIDED":"",t.shadowMapEnabled?"#define USE_SHADOWMAP":"",t.shadowMapEnabled?"#define "+c:"",t.sizeAttenuation?"#define USE_SIZEATTENUATION":"",t.numLightProbes>0?"#define USE_LIGHT_PROBES":"",t.logarithmicDepthBuffer?"#define USE_LOGARITHMIC_DEPTH_BUFFER":"",t.reversedDepthBuffer?"#define USE_REVERSED_DEPTH_BUFFER":"","uniform mat4 modelMatrix;","uniform mat4 modelViewMatrix;","uniform mat4 projectionMatrix;","uniform mat4 viewMatrix;","uniform mat3 normalMatrix;","uniform vec3 cameraPosition;","uniform bool isOrthographic;","#ifdef USE_INSTANCING","	attribute mat4 instanceMatrix;","#endif","#ifdef USE_INSTANCING_COLOR","	attribute vec3 instanceColor;","#endif","#ifdef USE_INSTANCING_MORPH","	uniform sampler2D morphTexture;","#endif","attribute vec3 position;","attribute vec3 normal;","attribute vec2 uv;","#ifdef USE_UV1","	attribute vec2 uv1;","#endif","#ifdef USE_UV2","	attribute vec2 uv2;","#endif","#ifdef USE_UV3","	attribute vec2 uv3;","#endif","#ifdef USE_TANGENT","	attribute vec4 tangent;","#endif","#if defined( USE_COLOR_ALPHA )","	attribute vec4 color;","#elif defined( USE_COLOR )","	attribute vec3 color;","#endif","#ifdef USE_SKINNING","	attribute vec4 skinIndex;","	attribute vec4 skinWeight;","#endif",`
`].filter(ho).join(`
`),d=[Nd(t),"#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,_,t.useFog&&t.fog?"#define USE_FOG":"",t.useFog&&t.fogExp2?"#define FOG_EXP2":"",t.alphaToCoverage?"#define ALPHA_TO_COVERAGE":"",t.map?"#define USE_MAP":"",t.matcap?"#define USE_MATCAP":"",t.envMap?"#define USE_ENVMAP":"",t.envMap?"#define "+l:"",t.envMap?"#define "+h:"",t.envMap?"#define "+m:"",u?"#define CUBEUV_TEXEL_WIDTH "+u.texelWidth:"",u?"#define CUBEUV_TEXEL_HEIGHT "+u.texelHeight:"",u?"#define CUBEUV_MAX_MIP "+u.maxMip+".0":"",t.lightMap?"#define USE_LIGHTMAP":"",t.aoMap?"#define USE_AOMAP":"",t.bumpMap?"#define USE_BUMPMAP":"",t.normalMap?"#define USE_NORMALMAP":"",t.normalMapObjectSpace?"#define USE_NORMALMAP_OBJECTSPACE":"",t.normalMapTangentSpace?"#define USE_NORMALMAP_TANGENTSPACE":"",t.packedNormalMap?"#define USE_PACKED_NORMALMAP":"",t.emissiveMap?"#define USE_EMISSIVEMAP":"",t.anisotropy?"#define USE_ANISOTROPY":"",t.anisotropyMap?"#define USE_ANISOTROPYMAP":"",t.clearcoat?"#define USE_CLEARCOAT":"",t.clearcoatMap?"#define USE_CLEARCOATMAP":"",t.clearcoatRoughnessMap?"#define USE_CLEARCOAT_ROUGHNESSMAP":"",t.clearcoatNormalMap?"#define USE_CLEARCOAT_NORMALMAP":"",t.dispersion?"#define USE_DISPERSION":"",t.retroreflection?"#define USE_RETROREFLECTION":"",t.iridescence?"#define USE_IRIDESCENCE":"",t.iridescenceMap?"#define USE_IRIDESCENCEMAP":"",t.iridescenceThicknessMap?"#define USE_IRIDESCENCE_THICKNESSMAP":"",t.specularMap?"#define USE_SPECULARMAP":"",t.specularColorMap?"#define USE_SPECULAR_COLORMAP":"",t.specularIntensityMap?"#define USE_SPECULAR_INTENSITYMAP":"",t.roughnessMap?"#define USE_ROUGHNESSMAP":"",t.metalnessMap?"#define USE_METALNESSMAP":"",t.alphaMap?"#define USE_ALPHAMAP":"",t.alphaTest?"#define USE_ALPHATEST":"",t.alphaHash?"#define USE_ALPHAHASH":"",t.sheen?"#define USE_SHEEN":"",t.sheenColorMap?"#define USE_SHEEN_COLORMAP":"",t.sheenRoughnessMap?"#define USE_SHEEN_ROUGHNESSMAP":"",t.transmission?"#define USE_TRANSMISSION":"",t.transmissionMap?"#define USE_TRANSMISSIONMAP":"",t.thicknessMap?"#define USE_THICKNESSMAP":"",t.vertexTangents&&t.flatShading===!1?"#define USE_TANGENT":"",t.vertexColors||t.instancingColor?"#define USE_COLOR":"",t.vertexAlphas||t.batchingColor?"#define USE_COLOR_ALPHA":"",t.vertexUv1s?"#define USE_UV1":"",t.vertexUv2s?"#define USE_UV2":"",t.vertexUv3s?"#define USE_UV3":"",t.pointsUvs?"#define USE_POINTS_UV":"",t.gradientMap?"#define USE_GRADIENTMAP":"",t.flatShading?"#define FLAT_SHADED":"",t.doubleSided?"#define DOUBLE_SIDED":"",t.flipSided?"#define FLIP_SIDED":"",t.shadowMapEnabled?"#define USE_SHADOWMAP":"",t.shadowMapEnabled?"#define "+c:"",t.premultipliedAlpha?"#define PREMULTIPLIED_ALPHA":"",t.numLightProbes>0?"#define USE_LIGHT_PROBES":"",t.numLightProbeGrids>0?"#define USE_LIGHT_PROBES_GRID":"",t.decodeVideoTexture?"#define DECODE_VIDEO_TEXTURE":"",t.decodeVideoTextureEmissive?"#define DECODE_VIDEO_TEXTURE_EMISSIVE":"",t.logarithmicDepthBuffer?"#define USE_LOGARITHMIC_DEPTH_BUFFER":"",t.reversedDepthBuffer?"#define USE_REVERSED_DEPTH_BUFFER":"","uniform mat4 viewMatrix;","uniform vec3 cameraPosition;","uniform bool isOrthographic;",t.toneMapping!==ni?"#define TONE_MAPPING":"",t.toneMapping!==ni?tt.tonemapping_pars_fragment:"",t.toneMapping!==ni?ov("toneMapping",t.toneMapping):"",t.dithering?"#define DITHERING":"",t.opaque?"#define OPAQUE":"",tt.colorspace_pars_fragment,rv("linearToOutputTexel",t.outputColorSpace),av(),t.useDepthPacking?"#define DEPTH_PACKING "+t.depthPacking:"",`
`].filter(ho).join(`
`)),o=du(o),o=Ld(o,t),o=kd(o,t),a=du(a),a=Ld(a,t),a=kd(a,t),o=Dd(o),a=Dd(a),t.isRawShaderMaterial!==!0&&(S=`#version 300 es
`,f=[g,"#define attribute in","#define varying out","#define texture2D texture"].join(`
`)+`
`+f,d=["#define varying in",t.glslVersion===Yc?"":"layout(location = 0) out highp vec4 pc_fragColor;",t.glslVersion===Yc?"":"#define gl_FragColor pc_fragColor","#define gl_FragDepthEXT gl_FragDepth","#define texture2D texture","#define textureCube texture","#define texture2DProj textureProj","#define texture2DLodEXT textureLod","#define texture2DProjLodEXT textureProjLod","#define textureCubeLodEXT textureLod","#define texture2DGradEXT textureGrad","#define texture2DProjGradEXT textureProjGrad","#define textureCubeGradEXT textureGrad"].join(`
`)+`
`+d);let P=S+f+o,w=S+d+a,T=Cd(r,r.VERTEX_SHADER,P),A=Cd(r,r.FRAGMENT_SHADER,w);r.attachShader(x,T),r.attachShader(x,A),t.index0AttributeName!==void 0?r.bindAttribLocation(x,0,t.index0AttributeName):t.hasPositionAttribute===!0&&r.bindAttribLocation(x,0,"position"),r.linkProgram(x);function D(C){if(n.debug.checkShaderErrors){let k=r.getProgramInfoLog(x)||"",L=r.getShaderInfoLog(T)||"",U=r.getShaderInfoLog(A)||"",F=k.trim(),H=L.trim(),q=U.trim(),ee=!0,J=!0;if(r.getProgramParameter(x,r.LINK_STATUS)===!1)if(ee=!1,typeof n.debug.onShaderError=="function")n.debug.onShaderError(r,x,T,A);else{let ne=Id(r,T,"vertex"),ce=Id(r,A,"fragment");Xe("WebGLProgram: Shader Error "+r.getError()+" - VALIDATE_STATUS "+r.getProgramParameter(x,r.VALIDATE_STATUS)+`

Material Name: `+C.name+`
Material Type: `+C.type+`

Program Info Log: `+F+`
`+ne+`
`+ce)}else F!==""?Ge("WebGLProgram: Program Info Log:",F):(H===""||q==="")&&(J=!1);J&&(C.diagnostics={runnable:ee,programLog:F,vertexShader:{log:H,prefix:f},fragmentShader:{log:q,prefix:d}})}r.deleteShader(T),r.deleteShader(A),p=new ps(r,x),M=uv(r,x)}let p;this.getUniforms=function(){return p===void 0&&D(this),p};let M;this.getAttributes=function(){return M===void 0&&D(this),M};let y=t.rendererExtensionParallelShaderCompile===!1;return this.isReady=function(){return y===!1&&(y=r.getProgramParameter(x,ev)),y},this.destroy=function(){i.releaseStatesOfProgram(this),r.deleteProgram(x),this.program=void 0},this.type=t.shaderType,this.name=t.shaderName,this.id=tv++,this.cacheKey=e,this.usedTimes=1,this.program=x,this.vertexShader=T,this.fragmentShader=A,this}var Tv=0,fu=class{constructor(){this.shaderCache=new Map,this.materialCache=new Map}update(e,t,i){let r=this._getShaderCacheForMaterial(e);return r.has(t)===!1&&(r.add(t),t.usedTimes++),r.has(i)===!1&&(r.add(i),i.usedTimes++),this}remove(e){let t=this.materialCache.get(e);for(let i of t)i.usedTimes--,i.usedTimes===0&&this.shaderCache.delete(i.code);return this.materialCache.delete(e),this}getVertexShaderStage(e){return this._getShaderStage(e.vertexShader)}getFragmentShaderStage(e){return this._getShaderStage(e.fragmentShader)}dispose(){this.shaderCache.clear(),this.materialCache.clear()}_getShaderCacheForMaterial(e){let t=this.materialCache,i=t.get(e);return i===void 0&&(i=new Set,t.set(e,i)),i}_getShaderStage(e){let t=this.shaderCache,i=t.get(e);return i===void 0&&(i=new pu(e),t.set(e,i)),i}},pu=class{constructor(e){this.id=Tv++,this.code=e,this.usedTimes=0}};function Av(n){return n===Qi||n===ao||n===lo}function Rv(n,e,t,i,r,s){let o=new Jr,a=new fu,c=new Set,l=[],h=new Map,m=i.logarithmicDepthBuffer,u=i.precision,g={MeshDepthMaterial:"depth",MeshDistanceMaterial:"distance",MeshNormalMaterial:"normal",MeshBasicMaterial:"basic",MeshLambertMaterial:"lambert",MeshPhongMaterial:"phong",MeshToonMaterial:"toon",MeshStandardMaterial:"physical",MeshPhysicalMaterial:"physical",MeshMatcapMaterial:"matcap",LineBasicMaterial:"basic",LineDashedMaterial:"dashed",PointsMaterial:"points",ShadowMaterial:"shadow",SpriteMaterial:"sprite"};function _(p){return c.add(p),p===0?"uv":`uv${p}`}function x(p,M,y,C,k,L){let U=C.fog,F=k.geometry,H=p.isMeshStandardMaterial||p.isMeshLambertMaterial||p.isMeshPhongMaterial?C.environment:null,q=p.isMeshStandardMaterial||p.isMeshLambertMaterial&&!p.envMap||p.isMeshPhongMaterial&&!p.envMap,ee=e.get(p.envMap||H,q),J=ee&&ee.mapping===to?ee.image.height:null,ne=g[p.type];p.precision!==null&&(u=i.getMaxPrecision(p.precision),u!==p.precision&&Ge("WebGLProgram.getParameters:",p.precision,"not supported, using",u,"instead."));let ce=F.morphAttributes.position||F.morphAttributes.normal||F.morphAttributes.color,pe=ce!==void 0?ce.length:0,me=0;F.morphAttributes.position!==void 0&&(me=1),F.morphAttributes.normal!==void 0&&(me=2),F.morphAttributes.color!==void 0&&(me=3);let qe,Le,Je,se;if(ne){let vt=xi[ne];qe=vt.vertexShader,Le=vt.fragmentShader}else{qe=p.vertexShader,Le=p.fragmentShader;let vt=a.getVertexShaderStage(p),rt=a.getFragmentShaderStage(p);a.update(p,vt,rt),Je=vt.id,se=rt.id}let le=n.getRenderTarget(),Se=n.state.buffers.depth.getReversed(),ke=k.isInstancedMesh===!0,ye=k.isBatchedMesh===!0,je=!!p.map,Nt=!!p.matcap,Ye=!!ee,nt=!!p.aoMap,ft=!!p.lightMap,Qe=!!p.bumpMap&&p.wireframe===!1,Tt=!!p.normalMap,Bt=!!p.displacementMap,$t=!!p.emissiveMap,wt=!!p.metalnessMap,Ut=!!p.roughnessMap,W=p.anisotropy>0,Qt=p.clearcoat>0,at=p.dispersion>0,N=p.retroreflectivity>0,b=p.iridescence>0,Z=p.sheen>0,te=p.transmission>0,ae=W&&!!p.anisotropyMap,ve=Qt&&!!p.clearcoatMap,Me=Qt&&!!p.clearcoatNormalMap,R=Qt&&!!p.clearcoatRoughnessMap,I=b&&!!p.iridescenceMap,O=b&&!!p.iridescenceThicknessMap,z=Z&&!!p.sheenColorMap,X=Z&&!!p.sheenRoughnessMap,Y=!!p.specularMap,re=!!p.specularColorMap,fe=!!p.specularIntensityMap,ge=te&&!!p.transmissionMap,B=te&&!!p.thicknessMap,ue=!!p.gradientMap,oe=!!p.alphaMap,xe=p.alphaTest>0,_e=!!p.alphaHash,he=!!p.extensions,we=ni;p.toneMapped&&(le===null||le.isXRRenderTarget===!0)&&(we=n.toneMapping);let Re={shaderID:ne,shaderType:p.type,shaderName:p.name,vertexShader:qe,fragmentShader:Le,defines:p.defines,customVertexShaderID:Je,customFragmentShaderID:se,isRawShaderMaterial:p.isRawShaderMaterial===!0,glslVersion:p.glslVersion,precision:u,batching:ye,batchingColor:ye&&k._colorsTexture!==null,instancing:ke,instancingColor:ke&&k.instanceColor!==null,instancingMorph:ke&&k.morphTexture!==null,outputColorSpace:le===null?n.outputColorSpace:le.isXRRenderTarget===!0?le.texture.colorSpace:lt.workingColorSpace,alphaToCoverage:!!p.alphaToCoverage,map:je,matcap:Nt,envMap:Ye,envMapMode:Ye&&ee.mapping,envMapCubeUVHeight:J,aoMap:nt,lightMap:ft,bumpMap:Qe,normalMap:Tt,displacementMap:Bt,emissiveMap:$t,normalMapObjectSpace:Tt&&p.normalMapType===nd,normalMapTangentSpace:Tt&&p.normalMapType===gl,packedNormalMap:Tt&&p.normalMapType===gl&&Av(p.normalMap.format),metalnessMap:wt,roughnessMap:Ut,anisotropy:W,anisotropyMap:ae,clearcoat:Qt,clearcoatMap:ve,clearcoatNormalMap:Me,clearcoatRoughnessMap:R,dispersion:at,retroreflection:N,iridescence:b,iridescenceMap:I,iridescenceThicknessMap:O,sheen:Z,sheenColorMap:z,sheenRoughnessMap:X,specularMap:Y,specularColorMap:re,specularIntensityMap:fe,transmission:te,transmissionMap:ge,thicknessMap:B,gradientMap:ue,opaque:p.transparent===!1&&p.blending===as&&p.alphaToCoverage===!1,alphaMap:oe,alphaTest:xe,alphaHash:_e,combine:p.combine,mapUv:je&&_(p.map.channel),aoMapUv:nt&&_(p.aoMap.channel),lightMapUv:ft&&_(p.lightMap.channel),bumpMapUv:Qe&&_(p.bumpMap.channel),normalMapUv:Tt&&_(p.normalMap.channel),displacementMapUv:Bt&&_(p.displacementMap.channel),emissiveMapUv:$t&&_(p.emissiveMap.channel),metalnessMapUv:wt&&_(p.metalnessMap.channel),roughnessMapUv:Ut&&_(p.roughnessMap.channel),anisotropyMapUv:ae&&_(p.anisotropyMap.channel),clearcoatMapUv:ve&&_(p.clearcoatMap.channel),clearcoatNormalMapUv:Me&&_(p.clearcoatNormalMap.channel),clearcoatRoughnessMapUv:R&&_(p.clearcoatRoughnessMap.channel),iridescenceMapUv:I&&_(p.iridescenceMap.channel),iridescenceThicknessMapUv:O&&_(p.iridescenceThicknessMap.channel),sheenColorMapUv:z&&_(p.sheenColorMap.channel),sheenRoughnessMapUv:X&&_(p.sheenRoughnessMap.channel),specularMapUv:Y&&_(p.specularMap.channel),specularColorMapUv:re&&_(p.specularColorMap.channel),specularIntensityMapUv:fe&&_(p.specularIntensityMap.channel),transmissionMapUv:ge&&_(p.transmissionMap.channel),thicknessMapUv:B&&_(p.thicknessMap.channel),alphaMapUv:oe&&_(p.alphaMap.channel),vertexTangents:!!F.attributes.tangent&&(Tt||W),vertexNormals:!!F.attributes.normal,vertexColors:p.vertexColors,vertexAlphas:p.vertexColors===!0&&!!F.attributes.color&&F.attributes.color.itemSize===4,pointsUvs:k.isPoints===!0&&!!F.attributes.uv&&(je||oe),fog:!!U,useFog:p.fog===!0,fogExp2:!!U&&U.isFogExp2,flatShading:p.wireframe===!1&&(p.flatShading===!0||F.attributes.normal===void 0&&Tt===!1&&(p.isMeshLambertMaterial||p.isMeshPhongMaterial||p.isMeshStandardMaterial||p.isMeshPhysicalMaterial)),sizeAttenuation:p.sizeAttenuation===!0,logarithmicDepthBuffer:m,reversedDepthBuffer:Se,skinning:k.isSkinnedMesh===!0,hasPositionAttribute:F.attributes.position!==void 0,morphTargets:F.morphAttributes.position!==void 0,morphNormals:F.morphAttributes.normal!==void 0,morphColors:F.morphAttributes.color!==void 0,morphTargetsCount:pe,morphTextureStride:me,numSunLights:M.sun.length,numDirLights:M.directional.length,numPointLights:M.point.length,numSpotLights:M.spot.length,numSpotLightMaps:M.spotLightMap.length,numRectAreaLights:M.rectArea.length,numHemiLights:M.hemi.length,numSunLightShadows:M.sunShadowMap.length,numDirLightShadows:M.directionalShadowMap.length,numPointLightShadows:M.pointShadowMap.length,numSpotLightShadows:M.spotShadowMap.length,numSpotLightShadowsWithMaps:M.numSpotLightShadowsWithMaps,numLightProbes:M.numLightProbes,numLightProbeGrids:L.length,numClippingPlanes:s.numPlanes,numClipIntersection:s.numIntersection,dithering:p.dithering,shadowMapEnabled:n.shadowMap.enabled&&y.length>0,shadowMapType:n.shadowMap.type,toneMapping:we,decodeVideoTexture:je&&p.map.isVideoTexture===!0&&lt.getTransfer(p.map.colorSpace)===Mt,decodeVideoTextureEmissive:$t&&p.emissiveMap.isVideoTexture===!0&&lt.getTransfer(p.emissiveMap.colorSpace)===Mt,premultipliedAlpha:p.premultipliedAlpha,doubleSided:p.side===qn,flipSided:p.side===Rn,useDepthPacking:p.depthPacking>=0,depthPacking:p.depthPacking||0,index0AttributeName:p.index0AttributeName,extensionClipCullDistance:he&&p.extensions.clipCullDistance===!0&&t.has("WEBGL_clip_cull_distance"),extensionMultiDraw:(he&&p.extensions.multiDraw===!0||ye)&&t.has("WEBGL_multi_draw"),rendererExtensionParallelShaderCompile:t.has("KHR_parallel_shader_compile"),customProgramCacheKey:p.customProgramCacheKey()};return Re.vertexUv1s=c.has(1),Re.vertexUv2s=c.has(2),Re.vertexUv3s=c.has(3),c.clear(),Re}function f(p){let M=[];if(p.shaderID?M.push(p.shaderID):(M.push(p.customVertexShaderID),M.push(p.customFragmentShaderID)),p.defines!==void 0)for(let y in p.defines)M.push(y),M.push(p.defines[y]);return p.isRawShaderMaterial===!1&&(d(M,p),S(M,p),M.push(n.outputColorSpace)),M.push(p.customProgramCacheKey),M.join()}function d(p,M){p.push(M.precision),p.push(M.outputColorSpace),p.push(M.envMapMode),p.push(M.envMapCubeUVHeight),p.push(M.mapUv),p.push(M.alphaMapUv),p.push(M.lightMapUv),p.push(M.aoMapUv),p.push(M.bumpMapUv),p.push(M.normalMapUv),p.push(M.displacementMapUv),p.push(M.emissiveMapUv),p.push(M.metalnessMapUv),p.push(M.roughnessMapUv),p.push(M.anisotropyMapUv),p.push(M.clearcoatMapUv),p.push(M.clearcoatNormalMapUv),p.push(M.clearcoatRoughnessMapUv),p.push(M.iridescenceMapUv),p.push(M.iridescenceThicknessMapUv),p.push(M.sheenColorMapUv),p.push(M.sheenRoughnessMapUv),p.push(M.specularMapUv),p.push(M.specularColorMapUv),p.push(M.specularIntensityMapUv),p.push(M.transmissionMapUv),p.push(M.thicknessMapUv),p.push(M.combine),p.push(M.fogExp2),p.push(M.sizeAttenuation),p.push(M.morphTargetsCount),p.push(M.morphAttributeCount),p.push(M.numSunLights),p.push(M.numDirLights),p.push(M.numPointLights),p.push(M.numSpotLights),p.push(M.numSpotLightMaps),p.push(M.numHemiLights),p.push(M.numRectAreaLights),p.push(M.numSunLightShadows),p.push(M.numDirLightShadows),p.push(M.numPointLightShadows),p.push(M.numSpotLightShadows),p.push(M.numSpotLightShadowsWithMaps),p.push(M.numLightProbes),p.push(M.shadowMapType),p.push(M.toneMapping),p.push(M.numClippingPlanes),p.push(M.numClipIntersection),p.push(M.depthPacking)}function S(p,M){o.disableAll(),M.instancing&&o.enable(0),M.instancingColor&&o.enable(1),M.instancingMorph&&o.enable(2),M.matcap&&o.enable(3),M.envMap&&o.enable(4),M.normalMapObjectSpace&&o.enable(5),M.normalMapTangentSpace&&o.enable(6),M.clearcoat&&o.enable(7),M.iridescence&&o.enable(8),M.alphaTest&&o.enable(9),M.vertexColors&&o.enable(10),M.vertexAlphas&&o.enable(11),M.vertexUv1s&&o.enable(12),M.vertexUv2s&&o.enable(13),M.vertexUv3s&&o.enable(14),M.vertexTangents&&o.enable(15),M.anisotropy&&o.enable(16),M.alphaHash&&o.enable(17),M.batching&&o.enable(18),M.dispersion&&o.enable(19),M.retroreflection&&o.enable(24),M.batchingColor&&o.enable(20),M.gradientMap&&o.enable(21),M.packedNormalMap&&o.enable(22),M.vertexNormals&&o.enable(23),p.push(o.mask),o.disableAll(),M.fog&&o.enable(0),M.useFog&&o.enable(1),M.flatShading&&o.enable(2),M.logarithmicDepthBuffer&&o.enable(3),M.reversedDepthBuffer&&o.enable(4),M.skinning&&o.enable(5),M.morphTargets&&o.enable(6),M.morphNormals&&o.enable(7),M.morphColors&&o.enable(8),M.premultipliedAlpha&&o.enable(9),M.shadowMapEnabled&&o.enable(10),M.doubleSided&&o.enable(11),M.flipSided&&o.enable(12),M.useDepthPacking&&o.enable(13),M.dithering&&o.enable(14),M.transmission&&o.enable(15),M.sheen&&o.enable(16),M.opaque&&o.enable(17),M.pointsUvs&&o.enable(18),M.decodeVideoTexture&&o.enable(19),M.decodeVideoTextureEmissive&&o.enable(20),M.alphaToCoverage&&o.enable(21),M.numLightProbeGrids>0&&o.enable(22),M.hasPositionAttribute&&o.enable(23),p.push(o.mask)}function P(p){let M=g[p.type],y;if(M){let C=xi[M];y=gd.clone(C.uniforms)}else y=p.uniforms;return y}function w(p,M){let y=h.get(M);return y!==void 0?++y.usedTimes:(y=new Ev(n,M,p,r),l.push(y),h.set(M,y)),y}function T(p){if(--p.usedTimes===0){let M=l.indexOf(p);l[M]=l[l.length-1],l.pop(),h.delete(p.cacheKey),p.destroy()}}function A(p){a.remove(p)}function D(){a.dispose()}return{getParameters:x,getProgramCacheKey:f,getUniforms:P,acquireProgram:w,releaseProgram:T,releaseShaderCache:A,programs:l,dispose:D}}function Cv(){let n=new WeakMap;function e(o){return n.has(o)}function t(o){let a=n.get(o);return a===void 0&&(a={},n.set(o,a)),a}function i(o){n.delete(o)}function r(o,a,c){n.get(o)[a]=c}function s(){n=new WeakMap}return{has:e,get:t,remove:i,update:r,dispose:s}}function Pv(n,e){return n.groupOrder!==e.groupOrder?n.groupOrder-e.groupOrder:n.renderOrder!==e.renderOrder?n.renderOrder-e.renderOrder:n.material.id!==e.material.id?n.material.id-e.material.id:n.materialVariant!==e.materialVariant?n.materialVariant-e.materialVariant:n.z!==e.z?n.z-e.z:n.id-e.id}function Ud(n,e){return n.groupOrder!==e.groupOrder?n.groupOrder-e.groupOrder:n.renderOrder!==e.renderOrder?n.renderOrder-e.renderOrder:n.z!==e.z?e.z-n.z:n.id-e.id}function Fd(){let n=[],e=0,t=[],i=[],r=[];function s(){e=0,t.length=0,i.length=0,r.length=0}function o(u){let g=0;return u.isInstancedMesh&&(g+=2),u.isSkinnedMesh&&(g+=1),g}function a(u,g,_,x,f,d){let S=n[e];return S===void 0?(S={id:u.id,object:u,geometry:g,material:_,materialVariant:o(u),groupOrder:x,renderOrder:u.renderOrder,z:f,group:d},n[e]=S):(S.id=u.id,S.object=u,S.geometry=g,S.material=_,S.materialVariant=o(u),S.groupOrder=x,S.renderOrder=u.renderOrder,S.z=f,S.group=d),e++,S}function c(u,g,_,x,f,d,S){S.reversedDepth===!0&&(f=-f);let P=a(u,g,_,x,f,d);_.transmission>0?i.push(P):_.transparent===!0?r.push(P):t.push(P)}function l(u,g,_,x,f,d){let S=a(u,g,_,x,f,d);_.transmission>0?i.unshift(S):_.transparent===!0?r.unshift(S):t.unshift(S)}function h(u,g){t.length>1&&t.sort(u||Pv),i.length>1&&i.sort(g||Ud),r.length>1&&r.sort(g||Ud)}function m(){for(let u=e,g=n.length;u<g;u++){let _=n[u];if(_.id===null)break;_.id=null,_.object=null,_.geometry=null,_.material=null,_.group=null}}return{opaque:t,transmissive:i,transparent:r,init:s,push:c,unshift:l,finish:m,sort:h}}function Iv(){let n=new WeakMap;function e(i,r){let s=n.get(i),o;return s===void 0?(o=new Fd,n.set(i,[o])):r>=s.length?(o=new Fd,s.push(o)):o=s[r],o}function t(){n=new WeakMap}return{get:e,dispose:t}}function Lv(){let n={};return{get:function(e){if(n[e.id]!==void 0)return n[e.id];let t;switch(e.type){case"SunLight":case"DirectionalLight":t={direction:new V,color:new ze};break;case"SpotLight":t={position:new V,direction:new V,color:new ze,distance:0,coneCos:0,penumbraCos:0,decay:0};break;case"PointLight":t={position:new V,color:new ze,distance:0,decay:0};break;case"HemisphereLight":t={direction:new V,skyColor:new ze,groundColor:new ze};break;case"RectAreaLight":t={color:new ze,position:new V,halfWidth:new V,halfHeight:new V};break}return n[e.id]=t,t}}}function kv(){let n={};return{get:function(e){if(n[e.id]!==void 0)return n[e.id];let t;switch(e.type){case"SunLight":case"DirectionalLight":t={shadowIntensity:1,shadowBias:0,shadowNormalBias:0,shadowRadius:1,shadowMapSize:new He};break;case"SpotLight":t={shadowIntensity:1,shadowBias:0,shadowNormalBias:0,shadowRadius:1,shadowMapSize:new He};break;case"PointLight":t={shadowIntensity:1,shadowBias:0,shadowNormalBias:0,shadowRadius:1,shadowMapSize:new He,shadowCameraNear:1,shadowCameraFar:1e3};break}return n[e.id]=t,t}}}var Dv=0;function Nv(n,e){return(e.castShadow?2:0)-(n.castShadow?2:0)+(e.map?1:0)-(n.map?1:0)}function Uv(n){let e=new Lv,t=kv(),i={version:0,hash:{sunLength:-1,directionalLength:-1,pointLength:-1,spotLength:-1,rectAreaLength:-1,hemiLength:-1,numSunShadows:-1,numDirectionalShadows:-1,numPointShadows:-1,numSpotShadows:-1,numSpotMaps:-1,numLightProbes:-1},ambient:[0,0,0],probe:[],sun:[],sunShadow:[],sunShadowMap:[],sunShadowMatrix:[],sunShadowCascade:[],directional:[],directionalShadow:[],directionalShadowMap:[],directionalShadowMatrix:[],spot:[],spotLightMap:[],spotShadow:[],spotShadowMap:[],spotLightMatrix:[],rectArea:[],rectAreaLTC1:null,rectAreaLTC2:null,point:[],pointShadow:[],pointShadowMap:[],pointShadowMatrix:[],hemi:[],numSpotLightShadowsWithMaps:0,numLightProbes:0};for(let l=0;l<9;l++)i.probe.push(new V);let r=new V,s=new It,o=new It;function a(l){let h=0,m=0,u=0;for(let k=0;k<9;k++)i.probe[k].set(0,0,0);let g=0,_=0,x=0,f=0,d=0,S=0,P=0,w=0,T=0,A=0,D=0,p=0,M=0,y=0;l.sort(Nv);for(let k=0,L=l.length;k<L;k++){let U=l[k],F=U.color,H=U.intensity,q=U.distance,ee=null;if(U.shadow&&U.shadow.map&&(U.shadow.map.texture.format===Qi?ee=U.shadow.map.texture:ee=U.shadow.map.depthTexture||U.shadow.map.texture),U.isAmbientLight)h+=F.r*H,m+=F.g*H,u+=F.b*H;else if(U.isLightProbe){for(let J=0;J<9;J++)i.probe[J].addScaledVector(U.sh.coefficients[J],H);y++}else if(U.isSunLight){let J=e.get(U);if(J.color.copy(U.color).multiplyScalar(U.intensity),U.castShadow){let ne=U.shadow,ce=t.get(U);ce.shadowIntensity=ne.intensity,ce.shadowBias=ne.bias,ce.shadowNormalBias=ne.normalBias,ce.shadowRadius=ne.radius,ce.shadowMapSize.copy(ne.mapSize).multiply(ne.getFrameExtents()),i.sunShadow[_]=ce,i.sunShadowMap[_]=ee;let pe=ne.getViewportCount();for(let me=0;me<pe;me++)i.sunShadowMatrix[x+me]=ne.getMatrix(me),i.sunShadowCascade[x+me]=ne._cascadeData[me];x+=pe,_++}i.sun[g]=J,g++}else if(U.isDirectionalLight){let J=e.get(U);if(J.color.copy(U.color).multiplyScalar(U.intensity),U.castShadow){let ne=U.shadow,ce=t.get(U);ce.shadowIntensity=ne.intensity,ce.shadowBias=ne.bias,ce.shadowNormalBias=ne.normalBias,ce.shadowRadius=ne.radius,ce.shadowMapSize=ne.mapSize,i.directionalShadow[f]=ce,i.directionalShadowMap[f]=ee,i.directionalShadowMatrix[f]=U.shadow.matrix,T++}i.directional[f]=J,f++}else if(U.isSpotLight){let J=e.get(U);J.position.setFromMatrixPosition(U.matrixWorld),J.color.copy(F).multiplyScalar(H),J.distance=q,J.coneCos=Math.cos(U.angle),J.penumbraCos=Math.cos(U.angle*(1-U.penumbra)),J.decay=U.decay,i.spot[S]=J;let ne=U.shadow;if(U.map&&(i.spotLightMap[p]=U.map,p++,ne.updateMatrices(U),U.castShadow&&M++),i.spotLightMatrix[S]=ne.matrix,U.castShadow){let ce=t.get(U);ce.shadowIntensity=ne.intensity,ce.shadowBias=ne.bias,ce.shadowNormalBias=ne.normalBias,ce.shadowRadius=ne.radius,ce.shadowMapSize=ne.mapSize,i.spotShadow[S]=ce,i.spotShadowMap[S]=ee,D++}S++}else if(U.isRectAreaLight){let J=e.get(U);J.color.copy(F).multiplyScalar(H),J.halfWidth.set(U.width*.5,0,0),J.halfHeight.set(0,U.height*.5,0),i.rectArea[P]=J,P++}else if(U.isPointLight){let J=e.get(U);if(J.color.copy(U.color).multiplyScalar(U.intensity),J.distance=U.distance,J.decay=U.decay,U.castShadow){let ne=U.shadow,ce=t.get(U);ce.shadowIntensity=ne.intensity,ce.shadowBias=ne.bias,ce.shadowNormalBias=ne.normalBias,ce.shadowRadius=ne.radius,ce.shadowMapSize=ne.mapSize,ce.shadowCameraNear=ne.camera.near,ce.shadowCameraFar=ne.camera.far,i.pointShadow[d]=ce,i.pointShadowMap[d]=ee,i.pointShadowMatrix[d]=U.shadow.matrix,A++}i.point[d]=J,d++}else if(U.isHemisphereLight){let J=e.get(U);J.skyColor.copy(U.color).multiplyScalar(H),J.groundColor.copy(U.groundColor).multiplyScalar(H),i.hemi[w]=J,w++}}P>0&&(n.has("OES_texture_float_linear")===!0?(i.rectAreaLTC1=Te.LTC_FLOAT_1,i.rectAreaLTC2=Te.LTC_FLOAT_2):(i.rectAreaLTC1=Te.LTC_HALF_1,i.rectAreaLTC2=Te.LTC_HALF_2)),i.ambient[0]=h,i.ambient[1]=m,i.ambient[2]=u;let C=i.hash;(C.sunLength!==g||C.directionalLength!==f||C.pointLength!==d||C.spotLength!==S||C.rectAreaLength!==P||C.hemiLength!==w||C.numSunShadows!==_||C.numDirectionalShadows!==T||C.numPointShadows!==A||C.numSpotShadows!==D||C.numSpotMaps!==p||C.numLightProbes!==y)&&(i.sun.length=g,i.directional.length=f,i.spot.length=S,i.rectArea.length=P,i.point.length=d,i.hemi.length=w,i.sunShadow.length=_,i.sunShadowMap.length=_,i.sunShadowMatrix.length=x,i.sunShadowCascade.length=x,i.directionalShadow.length=T,i.directionalShadowMap.length=T,i.directionalShadowMatrix.length=T,i.pointShadow.length=A,i.pointShadowMap.length=A,i.pointShadowMatrix.length=A,i.spotShadow.length=D,i.spotShadowMap.length=D,i.spotLightMatrix.length=D+p-M,i.spotLightMap.length=p,i.numSpotLightShadowsWithMaps=M,i.numLightProbes=y,C.sunLength=g,C.directionalLength=f,C.pointLength=d,C.spotLength=S,C.rectAreaLength=P,C.hemiLength=w,C.numSunShadows=_,C.numDirectionalShadows=T,C.numPointShadows=A,C.numSpotShadows=D,C.numSpotMaps=p,C.numLightProbes=y,i.version=Dv++)}function c(l,h){let m=0,u=0,g=0,_=0,x=0,f=0,d=h.matrixWorldInverse;for(let S=0,P=l.length;S<P;S++){let w=l[S];if(w.isSunLight){let T=i.sun[m];T.direction.setFromMatrixPosition(w.matrixWorld),T.direction.transformDirection(d),m++}else if(w.isDirectionalLight){let T=i.directional[u];T.direction.setFromMatrixPosition(w.matrixWorld),r.setFromMatrixPosition(w.target.matrixWorld),T.direction.sub(r),T.direction.transformDirection(d),u++}else if(w.isSpotLight){let T=i.spot[_];T.position.setFromMatrixPosition(w.matrixWorld),T.position.applyMatrix4(d),T.direction.setFromMatrixPosition(w.matrixWorld),r.setFromMatrixPosition(w.target.matrixWorld),T.direction.sub(r),T.direction.transformDirection(d),_++}else if(w.isRectAreaLight){let T=i.rectArea[x];T.position.setFromMatrixPosition(w.matrixWorld),T.position.applyMatrix4(d),o.identity(),s.copy(w.matrixWorld),s.premultiply(d),o.extractRotation(s),T.halfWidth.set(w.width*.5,0,0),T.halfHeight.set(0,w.height*.5,0),T.halfWidth.applyMatrix4(o),T.halfHeight.applyMatrix4(o),x++}else if(w.isPointLight){let T=i.point[g];T.position.setFromMatrixPosition(w.matrixWorld),T.position.applyMatrix4(d),g++}else if(w.isHemisphereLight){let T=i.hemi[f];T.direction.setFromMatrixPosition(w.matrixWorld),T.direction.transformDirection(d),f++}}}return{setup:a,setupView:c,state:i}}function Od(n){let e=new Uv(n),t=[],i=[],r=[];function s(u){m.camera=u,t.length=0,i.length=0,r.length=0}function o(u){t.push(u)}function a(u){i.push(u)}function c(u){r.push(u)}function l(){e.setup(t)}function h(u){e.setupView(t,u)}let m={lightsArray:t,shadowsArray:i,lightProbeGridArray:r,camera:null,lights:e,transmissionRenderTarget:{},textureUnits:0};return{init:s,state:m,setupLights:l,setupLightsView:h,pushLight:o,pushShadow:a,pushLightProbeGrid:c}}function Fv(n){let e=new WeakMap;function t(r,s=0){let o=e.get(r),a;return o===void 0?(a=new Od(n),e.set(r,[a])):s>=o.length?(a=new Od(n),o.push(a)):a=o[s],a}function i(){e=new WeakMap}return{get:t,dispose:i}}var Ov=`void main() {
	gl_Position = vec4( position, 1.0 );
}`,Bv=`uniform sampler2D shadow_pass;
uniform vec2 resolution;
uniform float radius;
void main() {
	const float samples = float( VSM_SAMPLES );
	float mean = 0.0;
	float squared_mean = 0.0;
	float uvStride = samples <= 1.0 ? 0.0 : 2.0 / ( samples - 1.0 );
	float uvStart = samples <= 1.0 ? 0.0 : - 1.0;
	for ( float i = 0.0; i < samples; i ++ ) {
		float uvOffset = uvStart + i * uvStride;
		#ifdef HORIZONTAL_PASS
			vec2 distribution = texture2D( shadow_pass, ( gl_FragCoord.xy + vec2( uvOffset, 0.0 ) * radius ) / resolution ).rg;
			mean += distribution.x;
			squared_mean += distribution.y * distribution.y + distribution.x * distribution.x;
		#else
			float depth = texture2D( shadow_pass, ( gl_FragCoord.xy + vec2( 0.0, uvOffset ) * radius ) / resolution ).r;
			mean += depth;
			squared_mean += depth * depth;
		#endif
	}
	mean = mean / samples;
	squared_mean = squared_mean / samples;
	float std_dev = sqrt( max( 0.0, squared_mean - mean * mean ) );
	gl_FragColor = vec4( mean, std_dev, 0.0, 1.0 );
}`,zv=[new V(1,0,0),new V(-1,0,0),new V(0,1,0),new V(0,-1,0),new V(0,0,1),new V(0,0,-1)],Hv=[new V(0,-1,0),new V(0,-1,0),new V(0,0,1),new V(0,0,-1),new V(0,-1,0),new V(0,-1,0)],Bd=new It,uo=new V,au=new V;function Vv(n,e,t){let i=new Qr,r=new He,s=new He,o=new Ot,a=new ga,c=new xa,l={},h=t.maxTextureSize,m={[Zi]:Rn,[Rn]:Zi,[qn]:qn},u=new On({defines:{VSM_SAMPLES:8},uniforms:{shadow_pass:{value:null},resolution:{value:new He},radius:{value:4}},vertexShader:Ov,fragmentShader:Bv}),g=u.clone();g.defines.HORIZONTAL_PASS=1;let _=new Xt;_.setAttribute("position",new Pn(new Float32Array([-1,-1,.5,3,-1,.5,-1,3,.5]),3));let x=new xt(_,u),f=this;this.enabled=!1,this.autoUpdate=!0,this.needsUpdate=!1,this.type=eo;let d=this.type;this.render=function(A,D,p){if(f.enabled===!1||f.autoUpdate===!1&&f.needsUpdate===!1||A.length===0)return;this.type===Pa&&(Ge("WebGLShadowMap: PCFSoftShadowMap has been removed. Using PCFShadowMap instead."),this.type=eo);let M=n.getRenderTarget(),y=n.getActiveCubeFace(),C=n.getActiveMipmapLevel(),k=n.state;k.setBlending(mi),k.buffers.depth.getReversed()===!0?k.buffers.color.setClear(0,0,0,0):k.buffers.color.setClear(1,1,1,1),k.buffers.depth.setTest(!0),k.setScissorTest(!1);let L=d!==this.type;L&&D.traverse(function(U){U.material&&(Array.isArray(U.material)?U.material.forEach(F=>F.needsUpdate=!0):U.material.needsUpdate=!0)});for(let U=0,F=A.length;U<F;U++){let H=A[U],q=H.shadow;if(q===void 0){Ge("WebGLShadowMap:",H,"has no shadow.");continue}if(q.autoUpdate===!1&&q.needsUpdate===!1)continue;r.copy(q.mapSize);let ee=q.getFrameExtents();r.multiply(ee),s.copy(q.mapSize),(r.x>h||r.y>h)&&(r.x>h&&(s.x=Math.floor(h/ee.x),r.x=s.x*ee.x,q.mapSize.x=s.x),r.y>h&&(s.y=Math.floor(h/ee.y),r.y=s.y*ee.y,q.mapSize.y=s.y));let J=n.state.buffers.depth.getReversed();if(q.camera._reversedDepth=J,q.map===null||L===!0){if(q.map!==null&&(q.map.depthTexture!==null&&(q.map.depthTexture.dispose(),q.map.depthTexture=null),q.map.dispose()),this.type===os){if(H.isPointLight){Ge("WebGLShadowMap: VSM shadow maps are not supported for PointLights. Use PCF or BasicShadowMap instead.");continue}q.map=new In(r.x,r.y,{format:Qi,type:si,minFilter:gn,magFilter:gn,generateMipmaps:!1}),q.map.texture.name=H.name+".shadowMap",q.map.depthTexture=new Vi(r.x,r.y,ri),q.map.depthTexture.name=H.name+".shadowMapDepth",q.map.depthTexture.format=di,q.map.depthTexture.compareFunction=null,q.map.depthTexture.minFilter=hn,q.map.depthTexture.magFilter=hn}else H.isPointLight?(q.map=new Ml(r.x),q.map.depthTexture=new fa(r.x,ii)):(q.map=new In(r.x,r.y),q.map.depthTexture=new Vi(r.x,r.y,ii)),q.map.depthTexture.name=H.name+".shadowMap",q.map.depthTexture.format=di,this.type===eo?(q.map.depthTexture.compareFunction=J?vl:xl,q.map.depthTexture.minFilter=gn,q.map.depthTexture.magFilter=gn):(q.map.depthTexture.compareFunction=null,q.map.depthTexture.minFilter=hn,q.map.depthTexture.magFilter=hn);q.camera.updateProjectionMatrix()}q.map.isWebGLCubeRenderTarget!==!0&&(q.map.width!==r.x||q.map.height!==r.y)&&q.map.setSize(r.x,r.y);let ne=q.map.isWebGLCubeRenderTarget?6:q.getViewportCount();H.isPointLight!==!0&&q.updateMatrices(H,p);for(let ce=0;ce<ne;ce++){let pe=q.getCamera(ce);if(H.isPointLight){let me=q.camera,qe=q.matrix,Le=H.distance||me.far;Le!==me.far&&(me.far=Le,me.updateProjectionMatrix()),uo.setFromMatrixPosition(H.matrixWorld),me.position.copy(uo),au.copy(me.position),au.add(zv[ce]),me.up.copy(Hv[ce]),me.lookAt(au),me.updateMatrixWorld(),qe.makeTranslation(-uo.x,-uo.y,-uo.z),Bd.multiplyMatrices(me.projectionMatrix,me.matrixWorldInverse),q._frustum.setFromProjectionMatrix(Bd,me.coordinateSystem,me.reversedDepth)}if(q.map.isWebGLCubeRenderTarget)n.setRenderTarget(q.map,ce),n.clear();else{ce===0&&(n.setRenderTarget(q.map),n.clear());let me=q.getViewport(ce);o.set(s.x*me.x,s.y*me.y,s.x*me.z,s.y*me.w),k.viewport(o)}i=q.getFrustum(ce),w(D,p,pe,H,this.type)}q.isPointLightShadow!==!0&&this.type===os&&S(q,p),q.needsUpdate=!1}d=this.type,f.needsUpdate=!1,n.setRenderTarget(M,y,C)};function S(A,D){let p=e.update(x);u.defines.VSM_SAMPLES!==A.blurSamples&&(u.defines.VSM_SAMPLES=A.blurSamples,g.defines.VSM_SAMPLES=A.blurSamples,u.needsUpdate=!0,g.needsUpdate=!0),A.mapPass===null?A.mapPass=new In(r.x,r.y,{format:Qi,type:si}):(A.mapPass.width!==A.map.width||A.mapPass.height!==A.map.height)&&A.mapPass.setSize(A.map.width,A.map.height),u.uniforms.shadow_pass.value=A.map.depthTexture,u.uniforms.resolution.value.set(A.map.width,A.map.height),u.uniforms.radius.value=A.radius,n.setRenderTarget(A.mapPass),n.clear(),n.renderBufferDirect(D,null,p,u,x,null),g.uniforms.shadow_pass.value=A.mapPass.texture,g.uniforms.resolution.value.set(A.map.width,A.map.height),g.uniforms.radius.value=A.radius,n.setRenderTarget(A.map),n.clear(),n.renderBufferDirect(D,null,p,g,x,null)}function P(A,D,p,M){let y=null,C=p.isPointLight===!0?A.customDistanceMaterial:A.customDepthMaterial;if(C!==void 0)y=C;else if(y=p.isPointLight===!0?c:a,n.localClippingEnabled&&D.clipShadows===!0&&Array.isArray(D.clippingPlanes)&&D.clippingPlanes.length!==0||D.displacementMap&&D.displacementScale!==0||D.alphaMap&&D.alphaTest>0||D.map&&D.alphaTest>0||D.alphaToCoverage===!0){let k=y.uuid,L=D.uuid,U=l[k];U===void 0&&(U={},l[k]=U);let F=U[L];F===void 0&&(F=y.clone(),U[L]=F,D.addEventListener("dispose",T)),y=F}if(y.visible=D.visible,y.wireframe=D.wireframe,M===os?y.side=D.shadowSide!==null?D.shadowSide:D.side:y.side=D.shadowSide!==null?D.shadowSide:m[D.side],y.alphaMap=D.alphaMap,y.alphaTest=D.alphaToCoverage===!0?.5:D.alphaTest,y.map=D.map,y.clipShadows=D.clipShadows,y.clippingPlanes=D.clippingPlanes,y.clipIntersection=D.clipIntersection,y.displacementMap=D.displacementMap,y.displacementScale=D.displacementScale,y.displacementBias=D.displacementBias,y.wireframeLinewidth=D.wireframeLinewidth,y.linewidth=D.linewidth,p.isPointLight===!0&&y.isMeshDistanceMaterial===!0){let k=n.properties.get(y);k.light=p}return y}function w(A,D,p,M,y){if(A.visible===!1)return;if(A.layers.test(D.layers)&&(A.isMesh||A.isLine||A.isPoints)&&(A.castShadow||A.receiveShadow&&y===os)&&(!A.frustumCulled||A.intersectsFrustum(i))){A.modelViewMatrix.multiplyMatrices(p.matrixWorldInverse,A.matrixWorld);let L=e.update(A),U=A.material;if(Array.isArray(U)){let F=L.groups;for(let H=0,q=F.length;H<q;H++){let ee=F[H],J=U[ee.materialIndex];if(J&&J.visible){let ne=P(A,J,M,y);A.onBeforeShadow(n,A,D,p,L,ne,ee),n.renderBufferDirect(p,null,L,ne,A,ee),A.onAfterShadow(n,A,D,p,L,ne,ee)}}}else if(U.visible){let F=P(A,U,M,y);A.onBeforeShadow(n,A,D,p,L,F,null),n.renderBufferDirect(p,null,L,F,A,null),A.onAfterShadow(n,A,D,p,L,F,null)}}let k=A.children;for(let L=0,U=k.length;L<U;L++)w(k[L],D,p,M,y)}function T(A){A.target.removeEventListener("dispose",T);for(let p in l){let M=l[p],y=A.target.uuid;y in M&&(M[y].dispose(),delete M[y])}}}function Gv(n,e){function t(){let B=!1,ue=new Ot,oe=null,xe=new Ot(0,0,0,0);return{setMask:function(_e){oe!==_e&&!B&&(n.colorMask(_e,_e,_e,_e),oe=_e)},setLocked:function(_e){B=_e},setClear:function(_e,he,we,Re,vt){vt===!0&&(_e*=Re,he*=Re,we*=Re),ue.set(_e,he,we,Re),xe.equals(ue)===!1&&(n.clearColor(_e,he,we,Re),xe.copy(ue))},reset:function(){B=!1,oe=null,xe.set(-1,0,0,0)}}}function i(){let B=!1,ue=!1,oe=null,xe=null,_e=null;return{setReversed:function(he){if(ue!==he){let we=e.get("EXT_clip_control");he?we.clipControlEXT(we.LOWER_LEFT_EXT,we.ZERO_TO_ONE_EXT):we.clipControlEXT(we.LOWER_LEFT_EXT,we.NEGATIVE_ONE_TO_ONE_EXT),ue=he;let Re=_e;_e=null,this.setClear(Re)}},getReversed:function(){return ue},setTest:function(he){he?le(n.DEPTH_TEST):Se(n.DEPTH_TEST)},setMask:function(he){oe!==he&&!B&&(n.depthMask(he),oe=he)},setFunc:function(he){if(ue&&(he=pd[he]),xe!==he){switch(he){case Jo:n.depthFunc(n.NEVER);break;case jo:n.depthFunc(n.ALWAYS);break;case Qo:n.depthFunc(n.LESS);break;case qr:n.depthFunc(n.LEQUAL);break;case ea:n.depthFunc(n.EQUAL);break;case ta:n.depthFunc(n.GEQUAL);break;case na:n.depthFunc(n.GREATER);break;case ia:n.depthFunc(n.NOTEQUAL);break;default:n.depthFunc(n.LEQUAL)}xe=he}},setLocked:function(he){B=he},setClear:function(he){_e!==he&&(_e=he,ue&&(he=1-he),n.clearDepth(he))},reset:function(){B=!1,oe=null,xe=null,_e=null,ue=!1}}}function r(){let B=!1,ue=null,oe=null,xe=null,_e=null,he=null,we=null,Re=null,vt=null;return{setTest:function(rt){B||(rt?le(n.STENCIL_TEST):Se(n.STENCIL_TEST))},setMask:function(rt){ue!==rt&&!B&&(n.stencilMask(rt),ue=rt)},setFunc:function(rt,Ft,wn){(oe!==rt||xe!==Ft||_e!==wn)&&(n.stencilFunc(rt,Ft,wn),oe=rt,xe=Ft,_e=wn)},setOp:function(rt,Ft,wn){(he!==rt||we!==Ft||Re!==wn)&&(n.stencilOp(rt,Ft,wn),he=rt,we=Ft,Re=wn)},setLocked:function(rt){B=rt},setClear:function(rt){vt!==rt&&(n.clearStencil(rt),vt=rt)},reset:function(){B=!1,ue=null,oe=null,xe=null,_e=null,he=null,we=null,Re=null,vt=null}}}let s=new t,o=new i,a=new r,c=new WeakMap,l=new WeakMap,h={},m={},u={},g=new WeakMap,_=[],x=null,f=!1,d=null,S=null,P=null,w=null,T=null,A=null,D=null,p=new ze(0,0,0),M=0,y=!1,C=null,k=null,L=null,U=null,F=null,H=n.getParameter(n.MAX_COMBINED_TEXTURE_IMAGE_UNITS),q=!1,ee=0,J=n.getParameter(n.VERSION);J.indexOf("WebGL")!==-1?(ee=parseFloat(/^WebGL (\d)/.exec(J)[1]),q=ee>=1):J.indexOf("OpenGL ES")!==-1&&(ee=parseFloat(/^OpenGL ES (\d)/.exec(J)[1]),q=ee>=2);let ne=null,ce={},pe=n.getParameter(n.SCISSOR_BOX),me=n.getParameter(n.VIEWPORT),qe=new Ot().fromArray(pe),Le=new Ot().fromArray(me);function Je(B,ue,oe,xe){let _e=new Uint8Array(4),he=n.createTexture();n.bindTexture(B,he),n.texParameteri(B,n.TEXTURE_MIN_FILTER,n.NEAREST),n.texParameteri(B,n.TEXTURE_MAG_FILTER,n.NEAREST);for(let we=0;we<oe;we++)B===n.TEXTURE_3D||B===n.TEXTURE_2D_ARRAY?n.texImage3D(ue,0,n.RGBA,1,1,xe,0,n.RGBA,n.UNSIGNED_BYTE,_e):n.texImage2D(ue+we,0,n.RGBA,1,1,0,n.RGBA,n.UNSIGNED_BYTE,_e);return he}let se={};se[n.TEXTURE_2D]=Je(n.TEXTURE_2D,n.TEXTURE_2D,1),se[n.TEXTURE_CUBE_MAP]=Je(n.TEXTURE_CUBE_MAP,n.TEXTURE_CUBE_MAP_POSITIVE_X,6),se[n.TEXTURE_2D_ARRAY]=Je(n.TEXTURE_2D_ARRAY,n.TEXTURE_2D_ARRAY,1,1),se[n.TEXTURE_3D]=Je(n.TEXTURE_3D,n.TEXTURE_3D,1,1),s.setClear(0,0,0,1),o.setClear(1),a.setClear(0),le(n.DEPTH_TEST),o.setFunc(qr),Qe(!1),Tt(Ac),le(n.CULL_FACE),nt(mi);function le(B){h[B]!==!0&&(n.enable(B),h[B]=!0)}function Se(B){h[B]!==!1&&(n.disable(B),h[B]=!1)}function ke(B,ue){return u[B]!==ue?(n.bindFramebuffer(B,ue),u[B]=ue,B===n.DRAW_FRAMEBUFFER&&(u[n.FRAMEBUFFER]=ue),B===n.FRAMEBUFFER&&(u[n.DRAW_FRAMEBUFFER]=ue),!0):!1}function ye(B,ue){let oe=_,xe=!1;if(B){oe=g.get(ue),oe===void 0&&(oe=[],g.set(ue,oe));let _e=B.textures;if(oe.length!==_e.length||oe[0]!==n.COLOR_ATTACHMENT0){for(let he=0,we=_e.length;he<we;he++)oe[he]=n.COLOR_ATTACHMENT0+he;oe.length=_e.length,xe=!0}}else oe[0]!==n.BACK&&(oe[0]=n.BACK,xe=!0);xe&&n.drawBuffers(oe)}function je(B){return x!==B?(n.useProgram(B),x=B,!0):!1}let Nt={[pr]:n.FUNC_ADD,[Nh]:n.FUNC_SUBTRACT,[Uh]:n.FUNC_REVERSE_SUBTRACT};Nt[Fh]=n.MIN,Nt[Oh]=n.MAX;let Ye={[Bh]:n.ZERO,[zh]:n.ONE,[Hh]:n.SRC_COLOR,[Pc]:n.SRC_ALPHA,[$h]:n.SRC_ALPHA_SATURATE,[Xh]:n.DST_COLOR,[Gh]:n.DST_ALPHA,[Vh]:n.ONE_MINUS_SRC_COLOR,[Ic]:n.ONE_MINUS_SRC_ALPHA,[qh]:n.ONE_MINUS_DST_COLOR,[Wh]:n.ONE_MINUS_DST_ALPHA,[Yh]:n.CONSTANT_COLOR,[Zh]:n.ONE_MINUS_CONSTANT_COLOR,[Kh]:n.CONSTANT_ALPHA,[Jh]:n.ONE_MINUS_CONSTANT_ALPHA};function nt(B,ue,oe,xe,_e,he,we,Re,vt,rt){if(B===mi){f===!0&&(Se(n.BLEND),f=!1);return}if(f===!1&&(le(n.BLEND),f=!0),B!==Dh){if(B!==d||rt!==y){if((S!==pr||T!==pr)&&(n.blendEquation(n.FUNC_ADD),S=pr,T=pr),rt)switch(B){case as:n.blendFuncSeparate(n.ONE,n.ONE_MINUS_SRC_ALPHA,n.ONE,n.ONE_MINUS_SRC_ALPHA);break;case ls:n.blendFunc(n.ONE,n.ONE);break;case Rc:n.blendFuncSeparate(n.ZERO,n.ONE_MINUS_SRC_COLOR,n.ZERO,n.ONE);break;case Cc:n.blendFuncSeparate(n.DST_COLOR,n.ONE_MINUS_SRC_ALPHA,n.ZERO,n.ONE);break;default:Xe("WebGLState: Invalid blending: ",B);break}else switch(B){case as:n.blendFuncSeparate(n.SRC_ALPHA,n.ONE_MINUS_SRC_ALPHA,n.ONE,n.ONE_MINUS_SRC_ALPHA);break;case ls:n.blendFuncSeparate(n.SRC_ALPHA,n.ONE,n.ONE,n.ONE);break;case Rc:Xe("WebGLState: SubtractiveBlending requires material.premultipliedAlpha = true");break;case Cc:Xe("WebGLState: MultiplyBlending requires material.premultipliedAlpha = true");break;default:Xe("WebGLState: Invalid blending: ",B);break}P=null,w=null,A=null,D=null,p.set(0,0,0),M=0,d=B,y=rt}return}_e=_e||ue,he=he||oe,we=we||xe,(ue!==S||_e!==T)&&(n.blendEquationSeparate(Nt[ue],Nt[_e]),S=ue,T=_e),(oe!==P||xe!==w||he!==A||we!==D)&&(n.blendFuncSeparate(Ye[oe],Ye[xe],Ye[he],Ye[we]),P=oe,w=xe,A=he,D=we),(Re.equals(p)===!1||vt!==M)&&(n.blendColor(Re.r,Re.g,Re.b,vt),p.copy(Re),M=vt),d=B,y=!1}function ft(B,ue){B.side===qn?Se(n.CULL_FACE):le(n.CULL_FACE);let oe=B.side===Rn;ue&&(oe=!oe),Qe(oe),B.blending===as&&B.transparent===!1?nt(mi):nt(B.blending,B.blendEquation,B.blendSrc,B.blendDst,B.blendEquationAlpha,B.blendSrcAlpha,B.blendDstAlpha,B.blendColor,B.blendAlpha,B.premultipliedAlpha),o.setFunc(B.depthFunc),o.setTest(B.depthTest),o.setMask(B.depthWrite),s.setMask(B.colorWrite);let xe=B.stencilWrite;a.setTest(xe),xe&&(a.setMask(B.stencilWriteMask),a.setFunc(B.stencilFunc,B.stencilRef,B.stencilFuncMask),a.setOp(B.stencilFail,B.stencilZFail,B.stencilZPass)),$t(B.polygonOffset,B.polygonOffsetFactor,B.polygonOffsetUnits),B.alphaToCoverage===!0?le(n.SAMPLE_ALPHA_TO_COVERAGE):Se(n.SAMPLE_ALPHA_TO_COVERAGE)}function Qe(B){C!==B&&(B?n.frontFace(n.CW):n.frontFace(n.CCW),C=B)}function Tt(B){B!==Lh?(le(n.CULL_FACE),B!==k&&(B===Ac?n.cullFace(n.BACK):B===kh?n.cullFace(n.FRONT):n.cullFace(n.FRONT_AND_BACK))):Se(n.CULL_FACE),k=B}function Bt(B){B!==L&&(q&&n.lineWidth(B),L=B)}function $t(B,ue,oe){B?(le(n.POLYGON_OFFSET_FILL),(U!==ue||F!==oe)&&(U=ue,F=oe,o.getReversed()&&(ue=-ue),n.polygonOffset(ue,oe))):Se(n.POLYGON_OFFSET_FILL)}function wt(B){B?le(n.SCISSOR_TEST):Se(n.SCISSOR_TEST)}function Ut(B){B===void 0&&(B=n.TEXTURE0+H-1),ne!==B&&(n.activeTexture(B),ne=B)}function W(B,ue,oe){oe===void 0&&(ne===null?oe=n.TEXTURE0+H-1:oe=ne);let xe=ce[oe];xe===void 0&&(xe={type:void 0,texture:void 0},ce[oe]=xe),(xe.type!==B||xe.texture!==ue)&&(ne!==oe&&(n.activeTexture(oe),ne=oe),n.bindTexture(B,ue||se[B]),xe.type=B,xe.texture=ue)}function Qt(){let B=ce[ne];B!==void 0&&B.type!==void 0&&(n.bindTexture(B.type,null),B.type=void 0,B.texture=void 0)}function at(){try{n.compressedTexImage2D(...arguments)}catch(B){Xe("WebGLState:",B)}}function N(){try{n.compressedTexImage3D(...arguments)}catch(B){Xe("WebGLState:",B)}}function b(){try{n.texSubImage2D(...arguments)}catch(B){Xe("WebGLState:",B)}}function Z(){try{n.texSubImage3D(...arguments)}catch(B){Xe("WebGLState:",B)}}function te(){try{n.compressedTexSubImage2D(...arguments)}catch(B){Xe("WebGLState:",B)}}function ae(){try{n.compressedTexSubImage3D(...arguments)}catch(B){Xe("WebGLState:",B)}}function ve(){try{n.texStorage2D(...arguments)}catch(B){Xe("WebGLState:",B)}}function Me(){try{n.texStorage3D(...arguments)}catch(B){Xe("WebGLState:",B)}}function R(){try{n.texImage2D(...arguments)}catch(B){Xe("WebGLState:",B)}}function I(){try{n.texImage3D(...arguments)}catch(B){Xe("WebGLState:",B)}}function O(B){return m[B]!==void 0?m[B]:n.getParameter(B)}function z(B,ue){m[B]!==ue&&(n.pixelStorei(B,ue),m[B]=ue)}function X(B){qe.equals(B)===!1&&(n.scissor(B.x,B.y,B.z,B.w),qe.copy(B))}function Y(B){Le.equals(B)===!1&&(n.viewport(B.x,B.y,B.z,B.w),Le.copy(B))}function re(B,ue){let oe=l.get(ue);oe===void 0&&(oe=new WeakMap,l.set(ue,oe));let xe=oe.get(B);xe===void 0&&(xe=n.getUniformBlockIndex(ue,B.name),oe.set(B,xe))}function fe(B,ue){let xe=l.get(ue).get(B);c.get(ue)!==xe&&(n.uniformBlockBinding(ue,xe,B.__bindingPointIndex),c.set(ue,xe))}function ge(){n.disable(n.BLEND),n.disable(n.CULL_FACE),n.disable(n.DEPTH_TEST),n.disable(n.POLYGON_OFFSET_FILL),n.disable(n.SCISSOR_TEST),n.disable(n.STENCIL_TEST),n.disable(n.SAMPLE_ALPHA_TO_COVERAGE),n.blendEquation(n.FUNC_ADD),n.blendFunc(n.ONE,n.ZERO),n.blendFuncSeparate(n.ONE,n.ZERO,n.ONE,n.ZERO),n.blendColor(0,0,0,0),n.colorMask(!0,!0,!0,!0),n.clearColor(0,0,0,0),n.depthMask(!0),n.depthFunc(n.LESS),o.setReversed(!1),n.clearDepth(1),n.stencilMask(4294967295),n.stencilFunc(n.ALWAYS,0,4294967295),n.stencilOp(n.KEEP,n.KEEP,n.KEEP),n.clearStencil(0),n.cullFace(n.BACK),n.frontFace(n.CCW),n.polygonOffset(0,0),n.activeTexture(n.TEXTURE0),n.bindFramebuffer(n.FRAMEBUFFER,null),n.bindFramebuffer(n.DRAW_FRAMEBUFFER,null),n.bindFramebuffer(n.READ_FRAMEBUFFER,null),n.useProgram(null),n.lineWidth(1),n.scissor(0,0,n.canvas.width,n.canvas.height),n.viewport(0,0,n.canvas.width,n.canvas.height),n.pixelStorei(n.PACK_ALIGNMENT,4),n.pixelStorei(n.UNPACK_ALIGNMENT,4),n.pixelStorei(n.UNPACK_FLIP_Y_WEBGL,!1),n.pixelStorei(n.UNPACK_PREMULTIPLY_ALPHA_WEBGL,!1),n.pixelStorei(n.UNPACK_COLORSPACE_CONVERSION_WEBGL,n.BROWSER_DEFAULT_WEBGL),n.pixelStorei(n.PACK_ROW_LENGTH,0),n.pixelStorei(n.PACK_SKIP_PIXELS,0),n.pixelStorei(n.PACK_SKIP_ROWS,0),n.pixelStorei(n.UNPACK_ROW_LENGTH,0),n.pixelStorei(n.UNPACK_IMAGE_HEIGHT,0),n.pixelStorei(n.UNPACK_SKIP_PIXELS,0),n.pixelStorei(n.UNPACK_SKIP_ROWS,0),n.pixelStorei(n.UNPACK_SKIP_IMAGES,0),h={},m={},ne=null,ce={},u={},g=new WeakMap,_=[],x=null,f=!1,d=null,S=null,P=null,w=null,T=null,A=null,D=null,p=new ze(0,0,0),M=0,y=!1,C=null,k=null,L=null,U=null,F=null,qe.set(0,0,n.canvas.width,n.canvas.height),Le.set(0,0,n.canvas.width,n.canvas.height),s.reset(),o.reset(),a.reset()}return{buffers:{color:s,depth:o,stencil:a},enable:le,disable:Se,bindFramebuffer:ke,drawBuffers:ye,useProgram:je,setBlending:nt,setMaterial:ft,setFlipSided:Qe,setCullFace:Tt,setLineWidth:Bt,setPolygonOffset:$t,setScissorTest:wt,activeTexture:Ut,bindTexture:W,unbindTexture:Qt,compressedTexImage2D:at,compressedTexImage3D:N,texImage2D:R,texImage3D:I,pixelStorei:z,getParameter:O,updateUBOMapping:re,uniformBlockBinding:fe,texStorage2D:ve,texStorage3D:Me,texSubImage2D:b,texSubImage3D:Z,compressedTexSubImage2D:te,compressedTexSubImage3D:ae,scissor:X,viewport:Y,reset:ge}}function Wv(n,e,t,i,r,s,o){let a=e.has("WEBGL_multisampled_render_to_texture")?e.get("WEBGL_multisampled_render_to_texture"):null,c=typeof navigator>"u"?!1:/OculusBrowser/g.test(navigator.userAgent),l=new He,h=new WeakMap,m=new Set,u,g=new WeakMap,_=!1;try{_=typeof OffscreenCanvas<"u"&&new OffscreenCanvas(1,1).getContext("2d")!==null}catch{}function x(N,b){return _?new OffscreenCanvas(N,b):Ds("canvas")}function f(N,b,Z){let te=1,ae=at(N);if((ae.width>Z||ae.height>Z)&&(te=Z/Math.max(ae.width,ae.height)),te<1)if(typeof HTMLImageElement<"u"&&N instanceof HTMLImageElement||typeof HTMLCanvasElement<"u"&&N instanceof HTMLCanvasElement||typeof ImageBitmap<"u"&&N instanceof ImageBitmap||typeof VideoFrame<"u"&&N instanceof VideoFrame){let ve=Math.floor(te*ae.width),Me=Math.floor(te*ae.height);u===void 0&&(u=x(ve,Me));let R=b?x(ve,Me):u;return R.width=ve,R.height=Me,R.getContext("2d").drawImage(N,0,0,ve,Me),Ge("WebGLRenderer: Texture has been resized from ("+ae.width+"x"+ae.height+") to ("+ve+"x"+Me+")."),R}else return"data"in N&&Ge("WebGLRenderer: Image in DataTexture is too big ("+ae.width+"x"+ae.height+")."),N;return N}function d(N){return N.generateMipmaps}function S(N){n.generateMipmap(N)}function P(N){return N.isWebGLCubeRenderTarget?n.TEXTURE_CUBE_MAP:N.isWebGL3DRenderTarget?n.TEXTURE_3D:N.isWebGLArrayRenderTarget||N.isCompressedArrayTexture?n.TEXTURE_2D_ARRAY:n.TEXTURE_2D}function w(N,b,Z,te,ae,ve=!1){if(N!==null){if(n[N]!==void 0)return n[N];Ge("WebGLRenderer: Attempt to use non-existing WebGL internal format '"+N+"'")}let Me;te&&(Me=e.get("EXT_texture_norm16"),Me||Ge("WebGLRenderer: Unable to use normalized textures without EXT_texture_norm16 extension"));let R=b;if(b===n.RED&&(Z===n.FLOAT&&(R=n.R32F),Z===n.HALF_FLOAT&&(R=n.R16F),Z===n.UNSIGNED_BYTE&&(R=n.R8),Z===n.UNSIGNED_SHORT&&Me&&(R=Me.R16_EXT),Z===n.SHORT&&Me&&(R=Me.R16_SNORM_EXT)),b===n.RED_INTEGER&&(Z===n.UNSIGNED_BYTE&&(R=n.R8UI),Z===n.UNSIGNED_SHORT&&(R=n.R16UI),Z===n.UNSIGNED_INT&&(R=n.R32UI),Z===n.BYTE&&(R=n.R8I),Z===n.SHORT&&(R=n.R16I),Z===n.INT&&(R=n.R32I)),b===n.RG&&(Z===n.FLOAT&&(R=n.RG32F),Z===n.HALF_FLOAT&&(R=n.RG16F),Z===n.UNSIGNED_BYTE&&(R=n.RG8),Z===n.UNSIGNED_SHORT&&Me&&(R=Me.RG16_EXT),Z===n.SHORT&&Me&&(R=Me.RG16_SNORM_EXT)),b===n.RG_INTEGER&&(Z===n.UNSIGNED_BYTE&&(R=n.RG8UI),Z===n.UNSIGNED_SHORT&&(R=n.RG16UI),Z===n.UNSIGNED_INT&&(R=n.RG32UI),Z===n.BYTE&&(R=n.RG8I),Z===n.SHORT&&(R=n.RG16I),Z===n.INT&&(R=n.RG32I)),b===n.RGB_INTEGER&&(Z===n.UNSIGNED_BYTE&&(R=n.RGB8UI),Z===n.UNSIGNED_SHORT&&(R=n.RGB16UI),Z===n.UNSIGNED_INT&&(R=n.RGB32UI),Z===n.BYTE&&(R=n.RGB8I),Z===n.SHORT&&(R=n.RGB16I),Z===n.INT&&(R=n.RGB32I)),b===n.RGBA_INTEGER&&(Z===n.UNSIGNED_BYTE&&(R=n.RGBA8UI),Z===n.UNSIGNED_SHORT&&(R=n.RGBA16UI),Z===n.UNSIGNED_INT&&(R=n.RGBA32UI),Z===n.BYTE&&(R=n.RGBA8I),Z===n.SHORT&&(R=n.RGBA16I),Z===n.INT&&(R=n.RGBA32I)),b===n.RGB&&(Z===n.UNSIGNED_SHORT&&Me&&(R=Me.RGB16_EXT),Z===n.SHORT&&Me&&(R=Me.RGB16_SNORM_EXT),Z===n.UNSIGNED_INT_5_9_9_9_REV&&(R=n.RGB9_E5),Z===n.UNSIGNED_INT_10F_11F_11F_REV&&(R=n.R11F_G11F_B10F)),b===n.RGBA){let I=ve?ks:lt.getTransfer(ae);Z===n.FLOAT&&(R=n.RGBA32F),Z===n.HALF_FLOAT&&(R=n.RGBA16F),Z===n.UNSIGNED_BYTE&&(R=I===Mt?n.SRGB8_ALPHA8:n.RGBA8),Z===n.UNSIGNED_SHORT&&Me&&(R=Me.RGBA16_EXT),Z===n.SHORT&&Me&&(R=Me.RGBA16_SNORM_EXT),Z===n.UNSIGNED_SHORT_4_4_4_4&&(R=n.RGBA4),Z===n.UNSIGNED_SHORT_5_5_5_1&&(R=n.RGB5_A1)}return(R===n.R16F||R===n.R32F||R===n.RG16F||R===n.RG32F||R===n.RGBA16F||R===n.RGBA32F)&&e.get("EXT_color_buffer_float"),R}function T(N,b){let Z;return N?b===null||b===ii||b===us?Z=n.DEPTH24_STENCIL8:b===ri?Z=n.DEPTH32F_STENCIL8:b===cs&&(Z=n.DEPTH24_STENCIL8,Ge("DepthTexture: 16 bit depth attachment is not supported with stencil. Using 24-bit attachment.")):b===null||b===ii||b===us?Z=n.DEPTH_COMPONENT24:b===ri?Z=n.DEPTH_COMPONENT32F:b===cs&&(Z=n.DEPTH_COMPONENT16),Z}function A(N,b){return d(N)===!0||N.isFramebufferTexture&&N.minFilter!==hn&&N.minFilter!==gn?Math.log2(Math.max(b.width,b.height))+1:N.mipmaps!==void 0&&N.mipmaps.length>0?N.mipmaps.length:N.isCompressedTexture&&Array.isArray(N.image)?b.mipmaps.length:1}function D(N){let b=N.target;b.removeEventListener("dispose",D),M(b),b.isVideoTexture&&h.delete(b),b.isHTMLTexture&&m.delete(b)}function p(N){let b=N.target;b.removeEventListener("dispose",p),C(b)}function M(N){let b=i.get(N);if(b.__webglInit===void 0)return;let Z=N.source,te=g.get(Z);if(te){let ae=te[b.__cacheKey];ae.usedTimes--,ae.usedTimes===0&&y(N),Object.keys(te).length===0&&g.delete(Z)}i.remove(N)}function y(N){let b=i.get(N);n.deleteTexture(b.__webglTexture);let Z=N.source,te=g.get(Z);delete te[b.__cacheKey],o.memory.textures--}function C(N){let b=i.get(N);if(N.depthTexture&&(N.depthTexture.dispose(),i.remove(N.depthTexture)),N.isWebGLCubeRenderTarget)for(let te=0;te<6;te++){if(Array.isArray(b.__webglFramebuffer[te]))for(let ae=0;ae<b.__webglFramebuffer[te].length;ae++)n.deleteFramebuffer(b.__webglFramebuffer[te][ae]);else n.deleteFramebuffer(b.__webglFramebuffer[te]);b.__webglDepthbuffer&&n.deleteRenderbuffer(b.__webglDepthbuffer[te])}else{if(Array.isArray(b.__webglFramebuffer))for(let te=0;te<b.__webglFramebuffer.length;te++)n.deleteFramebuffer(b.__webglFramebuffer[te]);else n.deleteFramebuffer(b.__webglFramebuffer);if(b.__webglDepthbuffer&&n.deleteRenderbuffer(b.__webglDepthbuffer),b.__webglMultisampledFramebuffer&&n.deleteFramebuffer(b.__webglMultisampledFramebuffer),b.__webglColorRenderbuffer)for(let te=0;te<b.__webglColorRenderbuffer.length;te++)b.__webglColorRenderbuffer[te]&&n.deleteRenderbuffer(b.__webglColorRenderbuffer[te]);b.__webglDepthRenderbuffer&&n.deleteRenderbuffer(b.__webglDepthRenderbuffer)}let Z=N.textures;for(let te=0,ae=Z.length;te<ae;te++){let ve=i.get(Z[te]);ve.__webglTexture&&(n.deleteTexture(ve.__webglTexture),o.memory.textures--),i.remove(Z[te])}i.remove(N)}let k=0;function L(){k=0}function U(){return k}function F(N){k=N}function H(){let N=k;return N>=r.maxTextures&&Ge("WebGLTextures: Trying to use "+(N+1)+" texture units while this GPU supports only "+r.maxTextures),k+=1,N}function q(N){let b=[];return b.push(N.wrapS),b.push(N.wrapT),b.push(N.wrapR||0),b.push(N.magFilter),b.push(N.minFilter),b.push(N.anisotropy),b.push(N.internalFormat),b.push(N.format),b.push(N.type),b.push(N.generateMipmaps),b.push(N.premultiplyAlpha),b.push(N.flipY),b.push(N.unpackAlignment),b.push(N.colorSpace),b.join()}function ee(N,b){let Z=i.get(N);if(N.isVideoTexture&&W(N),N.isRenderTargetTexture===!1&&N.isExternalTexture!==!0&&N.version>0&&Z.__version!==N.version){let te=N.image;if(te===null)Ge("WebGLRenderer: Texture marked for update but no image data found.");else if(te.complete===!1)Ge("WebGLRenderer: Texture marked for update but image is incomplete");else{Se(Z,N,b);return}}else N.isExternalTexture&&(Z.__webglTexture=N.sourceTexture?N.sourceTexture:null);t.bindTexture(n.TEXTURE_2D,Z.__webglTexture,n.TEXTURE0+b)}function J(N,b){let Z=i.get(N);if(N.isRenderTargetTexture===!1&&N.version>0&&Z.__version!==N.version){Se(Z,N,b);return}else N.isExternalTexture&&(Z.__webglTexture=N.sourceTexture?N.sourceTexture:null);t.bindTexture(n.TEXTURE_2D_ARRAY,Z.__webglTexture,n.TEXTURE0+b)}function ne(N,b){let Z=i.get(N);if(N.isRenderTargetTexture===!1&&N.version>0&&Z.__version!==N.version){Se(Z,N,b);return}t.bindTexture(n.TEXTURE_3D,Z.__webglTexture,n.TEXTURE0+b)}function ce(N,b){let Z=i.get(N);if(N.isCubeDepthTexture!==!0&&N.version>0&&Z.__version!==N.version){ke(Z,N,b);return}t.bindTexture(n.TEXTURE_CUBE_MAP,Z.__webglTexture,n.TEXTURE0+b)}let pe={[ur]:n.REPEAT,[hi]:n.CLAMP_TO_EDGE,[ra]:n.MIRRORED_REPEAT},me={[hn]:n.NEAREST,[ed]:n.NEAREST_MIPMAP_NEAREST,[no]:n.NEAREST_MIPMAP_LINEAR,[gn]:n.LINEAR,[ka]:n.LINEAR_MIPMAP_NEAREST,[Ji]:n.LINEAR_MIPMAP_LINEAR},qe={[rd]:n.NEVER,[cd]:n.ALWAYS,[sd]:n.LESS,[xl]:n.LEQUAL,[od]:n.EQUAL,[vl]:n.GEQUAL,[ad]:n.GREATER,[ld]:n.NOTEQUAL};function Le(N,b){if(b.type===ri&&e.has("OES_texture_float_linear")===!1&&(b.magFilter===gn||b.magFilter===ka||b.magFilter===no||b.magFilter===Ji||b.minFilter===gn||b.minFilter===ka||b.minFilter===no||b.minFilter===Ji)&&Ge("WebGLRenderer: Unable to use linear filtering with floating point textures. OES_texture_float_linear not supported on this device."),n.texParameteri(N,n.TEXTURE_WRAP_S,pe[b.wrapS]),n.texParameteri(N,n.TEXTURE_WRAP_T,pe[b.wrapT]),(N===n.TEXTURE_3D||N===n.TEXTURE_2D_ARRAY)&&n.texParameteri(N,n.TEXTURE_WRAP_R,pe[b.wrapR]),n.texParameteri(N,n.TEXTURE_MAG_FILTER,me[b.magFilter]),n.texParameteri(N,n.TEXTURE_MIN_FILTER,me[b.minFilter]),b.compareFunction&&(n.texParameteri(N,n.TEXTURE_COMPARE_MODE,n.COMPARE_REF_TO_TEXTURE),n.texParameteri(N,n.TEXTURE_COMPARE_FUNC,qe[b.compareFunction])),e.has("EXT_texture_filter_anisotropic")===!0){if(b.magFilter===hn||b.minFilter!==no&&b.minFilter!==Ji||b.type===ri&&e.has("OES_texture_float_linear")===!1)return;if(b.anisotropy>1||i.get(b).__currentAnisotropy){let Z=e.get("EXT_texture_filter_anisotropic");n.texParameterf(N,Z.TEXTURE_MAX_ANISOTROPY_EXT,Math.min(b.anisotropy,r.getMaxAnisotropy())),i.get(b).__currentAnisotropy=b.anisotropy}}}function Je(N,b){let Z=!1;N.__webglInit===void 0&&(N.__webglInit=!0,b.addEventListener("dispose",D));let te=b.source,ae=g.get(te);ae===void 0&&(ae={},g.set(te,ae));let ve=q(b);if(ve!==N.__cacheKey){ae[ve]===void 0&&(ae[ve]={texture:n.createTexture(),usedTimes:0},o.memory.textures++,Z=!0),ae[ve].usedTimes++;let Me=ae[N.__cacheKey];Me!==void 0&&(ae[N.__cacheKey].usedTimes--,Me.usedTimes===0&&y(b)),N.__cacheKey=ve,N.__webglTexture=ae[ve].texture}return Z}function se(N,b,Z){return Math.floor(Math.floor(N/Z)/b)}function le(N,b,Z,te){let ve=N.updateRanges;if(ve.length===0)t.texSubImage2D(n.TEXTURE_2D,0,0,0,b.width,b.height,Z,te,b.data);else{ve.sort((z,X)=>z.start-X.start);let Me=0;for(let z=1;z<ve.length;z++){let X=ve[Me],Y=ve[z],re=X.start+X.count,fe=se(Y.start,b.width,4),ge=se(X.start,b.width,4);Y.start<=re+1&&fe===ge&&se(Y.start+Y.count-1,b.width,4)===fe?X.count=Math.max(X.count,Y.start+Y.count-X.start):(++Me,ve[Me]=Y)}ve.length=Me+1;let R=t.getParameter(n.UNPACK_ROW_LENGTH),I=t.getParameter(n.UNPACK_SKIP_PIXELS),O=t.getParameter(n.UNPACK_SKIP_ROWS);t.pixelStorei(n.UNPACK_ROW_LENGTH,b.width);for(let z=0,X=ve.length;z<X;z++){let Y=ve[z],re=Math.floor(Y.start/4),fe=Math.ceil(Y.count/4),ge=re%b.width,B=Math.floor(re/b.width),ue=fe,oe=1;t.pixelStorei(n.UNPACK_SKIP_PIXELS,ge),t.pixelStorei(n.UNPACK_SKIP_ROWS,B),t.texSubImage2D(n.TEXTURE_2D,0,ge,B,ue,oe,Z,te,b.data)}N.clearUpdateRanges(),t.pixelStorei(n.UNPACK_ROW_LENGTH,R),t.pixelStorei(n.UNPACK_SKIP_PIXELS,I),t.pixelStorei(n.UNPACK_SKIP_ROWS,O)}}function Se(N,b,Z){let te=n.TEXTURE_2D;(b.isDataArrayTexture||b.isCompressedArrayTexture)&&(te=n.TEXTURE_2D_ARRAY),b.isData3DTexture&&(te=n.TEXTURE_3D);let ae=Je(N,b),ve=b.source;t.bindTexture(te,N.__webglTexture,n.TEXTURE0+Z);let Me=i.get(ve);if(ve.version!==Me.__version||ae===!0){if(t.activeTexture(n.TEXTURE0+Z),(typeof ImageBitmap<"u"&&b.image instanceof ImageBitmap)===!1){let oe=lt.getPrimaries(lt.workingColorSpace),xe=b.colorSpace===Ci?null:lt.getPrimaries(b.colorSpace),_e=b.colorSpace===Ci||oe===xe?n.NONE:n.BROWSER_DEFAULT_WEBGL;t.pixelStorei(n.UNPACK_FLIP_Y_WEBGL,b.flipY),t.pixelStorei(n.UNPACK_PREMULTIPLY_ALPHA_WEBGL,b.premultiplyAlpha),t.pixelStorei(n.UNPACK_COLORSPACE_CONVERSION_WEBGL,_e)}t.pixelStorei(n.UNPACK_ALIGNMENT,b.unpackAlignment);let I=f(b.image,!1,r.maxTextureSize);I=Qt(b,I);let O=s.convert(b.format,b.colorSpace),z=s.convert(b.type),X=w(b.internalFormat,O,z,b.normalized,b.colorSpace,b.isVideoTexture);Le(te,b);let Y,re=b.mipmaps,fe=b.isVideoTexture!==!0,ge=Me.__version===void 0||ae===!0,B=ve.dataReady,ue=A(b,I);if(b.isDepthTexture)X=T(b.format===ji,b.type),ge&&(fe?t.texStorage2D(n.TEXTURE_2D,1,X,I.width,I.height):t.texImage2D(n.TEXTURE_2D,0,X,I.width,I.height,0,O,z,null));else if(b.isDataTexture)if(re.length>0){fe&&ge&&t.texStorage2D(n.TEXTURE_2D,ue,X,re[0].width,re[0].height);for(let oe=0,xe=re.length;oe<xe;oe++)Y=re[oe],fe?B&&t.texSubImage2D(n.TEXTURE_2D,oe,0,0,Y.width,Y.height,O,z,Y.data):t.texImage2D(n.TEXTURE_2D,oe,X,Y.width,Y.height,0,O,z,Y.data);b.generateMipmaps=!1}else fe?(ge&&t.texStorage2D(n.TEXTURE_2D,ue,X,I.width,I.height),B&&le(b,I,O,z)):t.texImage2D(n.TEXTURE_2D,0,X,I.width,I.height,0,O,z,I.data);else if(b.isCompressedTexture)if(b.isCompressedArrayTexture){fe&&ge&&t.texStorage3D(n.TEXTURE_2D_ARRAY,ue,X,re[0].width,re[0].height,I.depth);for(let oe=0,xe=re.length;oe<xe;oe++)if(Y=re[oe],b.format!==$n)if(O!==null)if(fe){if(B)if(b.layerUpdates.size>0){let _e=tu(Y.width,Y.height,b.format,b.type);for(let he of b.layerUpdates){let we=Y.data.subarray(he*_e/Y.data.BYTES_PER_ELEMENT,(he+1)*_e/Y.data.BYTES_PER_ELEMENT);t.compressedTexSubImage3D(n.TEXTURE_2D_ARRAY,oe,0,0,he,Y.width,Y.height,1,O,we)}}else t.compressedTexSubImage3D(n.TEXTURE_2D_ARRAY,oe,0,0,0,Y.width,Y.height,I.depth,O,Y.data)}else t.compressedTexImage3D(n.TEXTURE_2D_ARRAY,oe,X,Y.width,Y.height,I.depth,0,Y.data,0,0);else Ge("WebGLRenderer: Attempt to load unsupported compressed texture format in .uploadTexture()");else fe?B&&t.texSubImage3D(n.TEXTURE_2D_ARRAY,oe,0,0,0,Y.width,Y.height,I.depth,O,z,Y.data):t.texImage3D(n.TEXTURE_2D_ARRAY,oe,X,Y.width,Y.height,I.depth,0,O,z,Y.data);b.layerUpdates.size>0&&b.clearLayerUpdates()}else{fe&&ge&&t.texStorage2D(n.TEXTURE_2D,ue,X,re[0].width,re[0].height);for(let oe=0,xe=re.length;oe<xe;oe++)Y=re[oe],b.format!==$n?O!==null?fe?B&&t.compressedTexSubImage2D(n.TEXTURE_2D,oe,0,0,Y.width,Y.height,O,Y.data):t.compressedTexImage2D(n.TEXTURE_2D,oe,X,Y.width,Y.height,0,Y.data):Ge("WebGLRenderer: Attempt to load unsupported compressed texture format in .uploadTexture()"):fe?B&&t.texSubImage2D(n.TEXTURE_2D,oe,0,0,Y.width,Y.height,O,z,Y.data):t.texImage2D(n.TEXTURE_2D,oe,X,Y.width,Y.height,0,O,z,Y.data)}else if(b.isDataArrayTexture)if(fe){if(ge&&t.texStorage3D(n.TEXTURE_2D_ARRAY,ue,X,I.width,I.height,I.depth),B)if(b.layerUpdates.size>0){let oe=tu(I.width,I.height,b.format,b.type);for(let xe of b.layerUpdates){let _e=I.data.subarray(xe*oe/I.data.BYTES_PER_ELEMENT,(xe+1)*oe/I.data.BYTES_PER_ELEMENT);t.texSubImage3D(n.TEXTURE_2D_ARRAY,0,0,0,xe,I.width,I.height,1,O,z,_e)}b.clearLayerUpdates()}else t.texSubImage3D(n.TEXTURE_2D_ARRAY,0,0,0,0,I.width,I.height,I.depth,O,z,I.data)}else t.texImage3D(n.TEXTURE_2D_ARRAY,0,X,I.width,I.height,I.depth,0,O,z,I.data);else if(b.isData3DTexture)fe?(ge&&t.texStorage3D(n.TEXTURE_3D,ue,X,I.width,I.height,I.depth),B&&t.texSubImage3D(n.TEXTURE_3D,0,0,0,0,I.width,I.height,I.depth,O,z,I.data)):t.texImage3D(n.TEXTURE_3D,0,X,I.width,I.height,I.depth,0,O,z,I.data);else if(b.isFramebufferTexture){if(ge)if(fe)t.texStorage2D(n.TEXTURE_2D,ue,X,I.width,I.height);else{let oe=I.width,xe=I.height;for(let _e=0;_e<ue;_e++)t.texImage2D(n.TEXTURE_2D,_e,X,oe,xe,0,O,z,null),oe>>=1,xe>>=1}}else if(b.isHTMLTexture){if("texElementImage2D"in n){let oe=n.canvas;if(oe.hasAttribute("layoutsubtree")||oe.setAttribute("layoutsubtree","true"),I.parentNode!==oe){oe.appendChild(I),m.add(b),oe.onpaint=xe=>{let _e=xe.changedElements;for(let he of m)_e.includes(he.image)&&(he.needsUpdate=!0)},oe.requestPaint();return}if(n.texElementImage2D.length===3)n.texElementImage2D(n.TEXTURE_2D,n.RGBA8,I);else{let _e=n.RGBA,he=n.RGBA,we=n.UNSIGNED_BYTE;n.texElementImage2D(n.TEXTURE_2D,0,_e,he,we,I)}n.texParameteri(n.TEXTURE_2D,n.TEXTURE_MIN_FILTER,n.LINEAR),n.texParameteri(n.TEXTURE_2D,n.TEXTURE_WRAP_S,n.CLAMP_TO_EDGE),n.texParameteri(n.TEXTURE_2D,n.TEXTURE_WRAP_T,n.CLAMP_TO_EDGE)}}else if(re.length>0){if(fe&&ge){let oe=at(re[0]);t.texStorage2D(n.TEXTURE_2D,ue,X,oe.width,oe.height)}for(let oe=0,xe=re.length;oe<xe;oe++)Y=re[oe],fe?B&&t.texSubImage2D(n.TEXTURE_2D,oe,0,0,O,z,Y):t.texImage2D(n.TEXTURE_2D,oe,X,O,z,Y);b.generateMipmaps=!1}else if(fe){if(ge){let oe=at(I);t.texStorage2D(n.TEXTURE_2D,ue,X,oe.width,oe.height)}B&&t.texSubImage2D(n.TEXTURE_2D,0,0,0,O,z,I)}else t.texImage2D(n.TEXTURE_2D,0,X,O,z,I);d(b)&&S(te),Me.__version=ve.version,b.onUpdate&&b.onUpdate(b)}N.__version=b.version}function ke(N,b,Z){if(b.image.length!==6)return;let te=Je(N,b),ae=b.source;t.bindTexture(n.TEXTURE_CUBE_MAP,N.__webglTexture,n.TEXTURE0+Z);let ve=i.get(ae);if(ae.version!==ve.__version||te===!0){t.activeTexture(n.TEXTURE0+Z);let Me=lt.getPrimaries(lt.workingColorSpace),R=b.colorSpace===Ci?null:lt.getPrimaries(b.colorSpace),I=b.colorSpace===Ci||Me===R?n.NONE:n.BROWSER_DEFAULT_WEBGL;t.pixelStorei(n.UNPACK_FLIP_Y_WEBGL,b.flipY),t.pixelStorei(n.UNPACK_PREMULTIPLY_ALPHA_WEBGL,b.premultiplyAlpha),t.pixelStorei(n.UNPACK_ALIGNMENT,b.unpackAlignment),t.pixelStorei(n.UNPACK_COLORSPACE_CONVERSION_WEBGL,I);let O=b.isCompressedTexture||b.image[0].isCompressedTexture,z=b.image[0]&&b.image[0].isDataTexture,X=[];for(let he=0;he<6;he++)!O&&!z?X[he]=f(b.image[he],!0,r.maxCubemapSize):X[he]=z?b.image[he].image:b.image[he],X[he]=Qt(b,X[he]);let Y=X[0],re=s.convert(b.format,b.colorSpace),fe=s.convert(b.type),ge=w(b.internalFormat,re,fe,b.normalized,b.colorSpace),B=b.isVideoTexture!==!0,ue=ve.__version===void 0||te===!0,oe=ae.dataReady,xe=A(b,Y);Le(n.TEXTURE_CUBE_MAP,b);let _e;if(O){B&&ue&&t.texStorage2D(n.TEXTURE_CUBE_MAP,xe,ge,Y.width,Y.height);for(let he=0;he<6;he++){_e=X[he].mipmaps;for(let we=0;we<_e.length;we++){let Re=_e[we];b.format!==$n?re!==null?B?oe&&t.compressedTexSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,we,0,0,Re.width,Re.height,re,Re.data):t.compressedTexImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,we,ge,Re.width,Re.height,0,Re.data):Ge("WebGLRenderer: Attempt to load unsupported compressed texture format in .setTextureCube()"):B?oe&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,we,0,0,Re.width,Re.height,re,fe,Re.data):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,we,ge,Re.width,Re.height,0,re,fe,Re.data)}}}else{if(_e=b.mipmaps,B&&ue){_e.length>0&&xe++;let he=at(X[0]);t.texStorage2D(n.TEXTURE_CUBE_MAP,xe,ge,he.width,he.height)}for(let he=0;he<6;he++)if(z){B?oe&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,0,0,0,X[he].width,X[he].height,re,fe,X[he].data):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,0,ge,X[he].width,X[he].height,0,re,fe,X[he].data);for(let we=0;we<_e.length;we++){let vt=_e[we].image[he].image;B?oe&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,we+1,0,0,vt.width,vt.height,re,fe,vt.data):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,we+1,ge,vt.width,vt.height,0,re,fe,vt.data)}}else{B?oe&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,0,0,0,re,fe,X[he]):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,0,ge,re,fe,X[he]);for(let we=0;we<_e.length;we++){let Re=_e[we];B?oe&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,we+1,0,0,re,fe,Re.image[he]):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,we+1,ge,re,fe,Re.image[he])}}}d(b)&&S(n.TEXTURE_CUBE_MAP),ve.__version=ae.version,b.onUpdate&&b.onUpdate(b)}N.__version=b.version}function ye(N,b,Z,te,ae,ve){let Me=s.convert(Z.format,Z.colorSpace),R=s.convert(Z.type),I=w(Z.internalFormat,Me,R,Z.normalized,Z.colorSpace),O=i.get(b),z=i.get(Z);if(z.__renderTarget=b,!O.__hasExternalTextures){let X=Math.max(1,b.width>>ve),Y=Math.max(1,b.height>>ve);ae===n.TEXTURE_3D||ae===n.TEXTURE_2D_ARRAY?t.texImage3D(ae,ve,I,X,Y,b.depth,0,Me,R,null):t.texImage2D(ae,ve,I,X,Y,0,Me,R,null)}t.bindFramebuffer(n.FRAMEBUFFER,N),Ut(b)?a.framebufferTexture2DMultisampleEXT(n.FRAMEBUFFER,te,ae,z.__webglTexture,0,wt(b)):(ae===n.TEXTURE_2D||ae>=n.TEXTURE_CUBE_MAP_POSITIVE_X&&ae<=n.TEXTURE_CUBE_MAP_NEGATIVE_Z)&&n.framebufferTexture2D(n.FRAMEBUFFER,te,ae,z.__webglTexture,ve),t.bindFramebuffer(n.FRAMEBUFFER,null)}function je(N,b,Z){if(n.bindRenderbuffer(n.RENDERBUFFER,N),b.depthBuffer){let te=b.depthTexture,ae=te&&te.isDepthTexture?te.type:null,ve=T(b.stencilBuffer,ae),Me=b.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT;Ut(b)?a.renderbufferStorageMultisampleEXT(n.RENDERBUFFER,wt(b),ve,b.width,b.height):Z?n.renderbufferStorageMultisample(n.RENDERBUFFER,wt(b),ve,b.width,b.height):n.renderbufferStorage(n.RENDERBUFFER,ve,b.width,b.height),n.framebufferRenderbuffer(n.FRAMEBUFFER,Me,n.RENDERBUFFER,N)}else{let te=b.textures;for(let ae=0;ae<te.length;ae++){let ve=te[ae],Me=s.convert(ve.format,ve.colorSpace),R=s.convert(ve.type),I=w(ve.internalFormat,Me,R,ve.normalized,ve.colorSpace);Ut(b)?a.renderbufferStorageMultisampleEXT(n.RENDERBUFFER,wt(b),I,b.width,b.height):Z?n.renderbufferStorageMultisample(n.RENDERBUFFER,wt(b),I,b.width,b.height):n.renderbufferStorage(n.RENDERBUFFER,I,b.width,b.height)}}n.bindRenderbuffer(n.RENDERBUFFER,null)}function Nt(N,b,Z){let te=b.isWebGLCubeRenderTarget===!0;if(t.bindFramebuffer(n.FRAMEBUFFER,N),!(b.depthTexture&&b.depthTexture.isDepthTexture))throw new Error("THREE.WebGLTextures: renderTarget.depthTexture must be an instance of THREE.DepthTexture.");let ae=i.get(b.depthTexture);if(ae.__renderTarget=b,(!ae.__webglTexture||b.depthTexture.image.width!==b.width||b.depthTexture.image.height!==b.height)&&(b.depthTexture.image.width=b.width,b.depthTexture.image.height=b.height,b.depthTexture.needsUpdate=!0),te){if(ae.__webglInit===void 0&&(ae.__webglInit=!0,b.depthTexture.addEventListener("dispose",D)),ae.__webglTexture===void 0){ae.__webglTexture=n.createTexture(),t.bindTexture(n.TEXTURE_CUBE_MAP,ae.__webglTexture),Le(n.TEXTURE_CUBE_MAP,b.depthTexture);let O=s.convert(b.depthTexture.format),z=s.convert(b.depthTexture.type),X;b.depthTexture.format===di?X=n.DEPTH_COMPONENT24:b.depthTexture.format===ji&&(X=n.DEPTH24_STENCIL8);for(let Y=0;Y<6;Y++)n.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+Y,0,X,b.width,b.height,0,O,z,null)}}else ee(b.depthTexture,0);let ve=ae.__webglTexture,Me=wt(b),R=te?n.TEXTURE_CUBE_MAP_POSITIVE_X+Z:n.TEXTURE_2D,I=b.depthTexture.format===ji?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT;if(b.depthTexture.format===di)Ut(b)?a.framebufferTexture2DMultisampleEXT(n.FRAMEBUFFER,I,R,ve,0,Me):n.framebufferTexture2D(n.FRAMEBUFFER,I,R,ve,0);else if(b.depthTexture.format===ji)Ut(b)?a.framebufferTexture2DMultisampleEXT(n.FRAMEBUFFER,I,R,ve,0,Me):n.framebufferTexture2D(n.FRAMEBUFFER,I,R,ve,0);else throw new Error("THREE.WebGLTextures: Unknown depthTexture format.")}function Ye(N){let b=i.get(N),Z=N.isWebGLCubeRenderTarget===!0;if(b.__boundDepthTexture!==N.depthTexture){let te=N.depthTexture;if(b.__depthDisposeCallback&&b.__depthDisposeCallback(),te){let ae=()=>{delete b.__boundDepthTexture,delete b.__depthDisposeCallback,te.removeEventListener("dispose",ae)};te.addEventListener("dispose",ae),b.__depthDisposeCallback=ae}b.__boundDepthTexture=te}if(N.depthTexture&&!b.__autoAllocateDepthBuffer)if(Z)for(let te=0;te<6;te++)Nt(b.__webglFramebuffer[te],N,te);else{let te=N.texture.mipmaps;te&&te.length>0?Nt(b.__webglFramebuffer[0],N,0):Nt(b.__webglFramebuffer,N,0)}else if(Z){b.__webglDepthbuffer=[];for(let te=0;te<6;te++)if(t.bindFramebuffer(n.FRAMEBUFFER,b.__webglFramebuffer[te]),b.__webglDepthbuffer[te]===void 0)b.__webglDepthbuffer[te]=n.createRenderbuffer(),je(b.__webglDepthbuffer[te],N,!1);else{let ae=N.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT,ve=b.__webglDepthbuffer[te];n.bindRenderbuffer(n.RENDERBUFFER,ve),n.framebufferRenderbuffer(n.FRAMEBUFFER,ae,n.RENDERBUFFER,ve)}}else{let te=N.texture.mipmaps;if(te&&te.length>0?t.bindFramebuffer(n.FRAMEBUFFER,b.__webglFramebuffer[0]):t.bindFramebuffer(n.FRAMEBUFFER,b.__webglFramebuffer),b.__webglDepthbuffer===void 0)b.__webglDepthbuffer=n.createRenderbuffer(),je(b.__webglDepthbuffer,N,!1);else{let ae=N.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT,ve=b.__webglDepthbuffer;n.bindRenderbuffer(n.RENDERBUFFER,ve),n.framebufferRenderbuffer(n.FRAMEBUFFER,ae,n.RENDERBUFFER,ve)}}t.bindFramebuffer(n.FRAMEBUFFER,null)}function nt(N,b,Z){let te=i.get(N);b!==void 0&&ye(te.__webglFramebuffer,N,N.texture,n.COLOR_ATTACHMENT0,n.TEXTURE_2D,0),Z!==void 0&&Ye(N)}function ft(N){let b=N.texture,Z=i.get(N),te=i.get(b);N.addEventListener("dispose",p);let ae=N.textures,ve=N.isWebGLCubeRenderTarget===!0,Me=ae.length>1;if(Me||(te.__webglTexture===void 0&&(te.__webglTexture=n.createTexture()),te.__version=b.version,o.memory.textures++),ve){Z.__webglFramebuffer=[];for(let R=0;R<6;R++)if(b.mipmaps&&b.mipmaps.length>0){Z.__webglFramebuffer[R]=[];for(let I=0;I<b.mipmaps.length;I++)Z.__webglFramebuffer[R][I]=n.createFramebuffer()}else Z.__webglFramebuffer[R]=n.createFramebuffer()}else{if(b.mipmaps&&b.mipmaps.length>0){Z.__webglFramebuffer=[];for(let R=0;R<b.mipmaps.length;R++)Z.__webglFramebuffer[R]=n.createFramebuffer()}else Z.__webglFramebuffer=n.createFramebuffer();if(Me)for(let R=0,I=ae.length;R<I;R++){let O=i.get(ae[R]);O.__webglTexture===void 0&&(O.__webglTexture=n.createTexture(),o.memory.textures++)}if(N.samples>0&&Ut(N)===!1){Z.__webglMultisampledFramebuffer=n.createFramebuffer(),Z.__webglColorRenderbuffer=[],t.bindFramebuffer(n.FRAMEBUFFER,Z.__webglMultisampledFramebuffer);for(let R=0;R<ae.length;R++){let I=ae[R];Z.__webglColorRenderbuffer[R]=n.createRenderbuffer(),n.bindRenderbuffer(n.RENDERBUFFER,Z.__webglColorRenderbuffer[R]);let O=s.convert(I.format,I.colorSpace),z=s.convert(I.type),X=w(I.internalFormat,O,z,I.normalized,I.colorSpace,N.isXRRenderTarget===!0),Y=wt(N);n.renderbufferStorageMultisample(n.RENDERBUFFER,Y,X,N.width,N.height),n.framebufferRenderbuffer(n.FRAMEBUFFER,n.COLOR_ATTACHMENT0+R,n.RENDERBUFFER,Z.__webglColorRenderbuffer[R])}n.bindRenderbuffer(n.RENDERBUFFER,null),N.depthBuffer&&(Z.__webglDepthRenderbuffer=n.createRenderbuffer(),je(Z.__webglDepthRenderbuffer,N,!0)),t.bindFramebuffer(n.FRAMEBUFFER,null)}}if(ve){t.bindTexture(n.TEXTURE_CUBE_MAP,te.__webglTexture),Le(n.TEXTURE_CUBE_MAP,b);for(let R=0;R<6;R++)if(b.mipmaps&&b.mipmaps.length>0)for(let I=0;I<b.mipmaps.length;I++)ye(Z.__webglFramebuffer[R][I],N,b,n.COLOR_ATTACHMENT0,n.TEXTURE_CUBE_MAP_POSITIVE_X+R,I);else ye(Z.__webglFramebuffer[R],N,b,n.COLOR_ATTACHMENT0,n.TEXTURE_CUBE_MAP_POSITIVE_X+R,0);d(b)&&S(n.TEXTURE_CUBE_MAP),t.unbindTexture()}else if(Me){for(let R=0,I=ae.length;R<I;R++){let O=ae[R],z=i.get(O),X=n.TEXTURE_2D;(N.isWebGL3DRenderTarget||N.isWebGLArrayRenderTarget)&&(X=N.isWebGL3DRenderTarget?n.TEXTURE_3D:n.TEXTURE_2D_ARRAY),t.bindTexture(X,z.__webglTexture),Le(X,O),ye(Z.__webglFramebuffer,N,O,n.COLOR_ATTACHMENT0+R,X,0),d(O)&&S(X)}t.unbindTexture()}else{let R=n.TEXTURE_2D;if((N.isWebGL3DRenderTarget||N.isWebGLArrayRenderTarget)&&(R=N.isWebGL3DRenderTarget?n.TEXTURE_3D:n.TEXTURE_2D_ARRAY),t.bindTexture(R,te.__webglTexture),Le(R,b),b.mipmaps&&b.mipmaps.length>0)for(let I=0;I<b.mipmaps.length;I++)ye(Z.__webglFramebuffer[I],N,b,n.COLOR_ATTACHMENT0,R,I);else ye(Z.__webglFramebuffer,N,b,n.COLOR_ATTACHMENT0,R,0);d(b)&&S(R),t.unbindTexture()}N.depthBuffer&&Ye(N)}function Qe(N){let b=N.textures;for(let Z=0,te=b.length;Z<te;Z++){let ae=b[Z];if(d(ae)){let ve=P(N),Me=i.get(ae).__webglTexture;t.bindTexture(ve,Me),S(ve),t.unbindTexture()}}}let Tt=[],Bt=[];function $t(N){if(N.samples>0){if(Ut(N)===!1){let b=N.textures,Z=N.width,te=N.height,ae=n.COLOR_BUFFER_BIT,ve=N.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT,Me=i.get(N),R=b.length>1;if(R)for(let O=0;O<b.length;O++)t.bindFramebuffer(n.FRAMEBUFFER,Me.__webglMultisampledFramebuffer),n.framebufferRenderbuffer(n.FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.RENDERBUFFER,null),t.bindFramebuffer(n.FRAMEBUFFER,Me.__webglFramebuffer),n.framebufferTexture2D(n.DRAW_FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.TEXTURE_2D,null,0);t.bindFramebuffer(n.READ_FRAMEBUFFER,Me.__webglMultisampledFramebuffer);let I=N.texture.mipmaps;I&&I.length>0?t.bindFramebuffer(n.DRAW_FRAMEBUFFER,Me.__webglFramebuffer[0]):t.bindFramebuffer(n.DRAW_FRAMEBUFFER,Me.__webglFramebuffer);for(let O=0;O<b.length;O++){if(N.resolveDepthBuffer&&(N.depthBuffer&&(ae|=n.DEPTH_BUFFER_BIT),N.stencilBuffer&&N.resolveStencilBuffer&&(ae|=n.STENCIL_BUFFER_BIT)),R){n.framebufferRenderbuffer(n.READ_FRAMEBUFFER,n.COLOR_ATTACHMENT0,n.RENDERBUFFER,Me.__webglColorRenderbuffer[O]);let z=i.get(b[O]).__webglTexture;n.framebufferTexture2D(n.DRAW_FRAMEBUFFER,n.COLOR_ATTACHMENT0,n.TEXTURE_2D,z,0)}n.blitFramebuffer(0,0,Z,te,0,0,Z,te,ae,n.NEAREST),c===!0&&(Tt.length=0,Bt.length=0,Tt.push(n.COLOR_ATTACHMENT0+O),N.depthBuffer&&N.storeMultisampledDepthBuffer===!1&&(Tt.push(ve),Bt.push(ve),n.invalidateFramebuffer(n.DRAW_FRAMEBUFFER,Bt)),n.invalidateFramebuffer(n.READ_FRAMEBUFFER,Tt))}if(t.bindFramebuffer(n.READ_FRAMEBUFFER,null),t.bindFramebuffer(n.DRAW_FRAMEBUFFER,null),R)for(let O=0;O<b.length;O++){t.bindFramebuffer(n.FRAMEBUFFER,Me.__webglMultisampledFramebuffer),n.framebufferRenderbuffer(n.FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.RENDERBUFFER,Me.__webglColorRenderbuffer[O]);let z=i.get(b[O]).__webglTexture;t.bindFramebuffer(n.FRAMEBUFFER,Me.__webglFramebuffer),n.framebufferTexture2D(n.DRAW_FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.TEXTURE_2D,z,0)}t.bindFramebuffer(n.DRAW_FRAMEBUFFER,Me.__webglMultisampledFramebuffer)}else if(N.depthBuffer&&N.storeMultisampledDepthBuffer===!1&&c){let b=N.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT;n.invalidateFramebuffer(n.DRAW_FRAMEBUFFER,[b])}}}function wt(N){return Math.min(r.maxSamples,N.samples)}function Ut(N){let b=i.get(N);return N.samples>0&&e.has("WEBGL_multisampled_render_to_texture")===!0&&b.__useRenderToTexture!==!1}function W(N){let b=o.render.frame;h.get(N)!==b&&(h.set(N,b),N.update())}function Qt(N,b){let Z=N.colorSpace,te=N.format,ae=N.type;return N.isCompressedTexture===!0||N.isVideoTexture===!0||Z!==Ls&&Z!==Ci&&(lt.getTransfer(Z)===Mt?(te!==$n||ae!==Ln)&&Ge("WebGLTextures: sRGB encoded textures have to use RGBAFormat and UnsignedByteType."):Xe("WebGLTextures: Unsupported texture color space:",Z)),b}function at(N){return typeof HTMLImageElement<"u"&&N instanceof HTMLImageElement?(l.width=N.naturalWidth||N.width,l.height=N.naturalHeight||N.height):typeof VideoFrame<"u"&&N instanceof VideoFrame?(l.width=N.displayWidth,l.height=N.displayHeight):(l.width=N.width,l.height=N.height),l}this.allocateTextureUnit=H,this.resetTextureUnits=L,this.getTextureUnits=U,this.setTextureUnits=F,this.setTexture2D=ee,this.setTexture2DArray=J,this.setTexture3D=ne,this.setTextureCube=ce,this.rebindTextures=nt,this.setupRenderTarget=ft,this.updateRenderTargetMipmap=Qe,this.updateMultisampleRenderTarget=$t,this.setupDepthRenderbuffer=Ye,this.setupFrameBufferTexture=ye,this.useMultisampledRTT=Ut,this.isReversedDepthBuffer=function(){return t.buffers.depth.getReversed()}}function Xv(n,e){function t(i,r=Ci){let s,o=lt.getTransfer(r);if(i===Ln)return n.UNSIGNED_BYTE;if(i===Na)return n.UNSIGNED_SHORT_4_4_4_4;if(i===Ua)return n.UNSIGNED_SHORT_5_5_5_1;if(i===Gc)return n.UNSIGNED_INT_5_9_9_9_REV;if(i===Wc)return n.UNSIGNED_INT_10F_11F_11F_REV;if(i===Hc)return n.BYTE;if(i===Vc)return n.SHORT;if(i===cs)return n.UNSIGNED_SHORT;if(i===Da)return n.INT;if(i===ii)return n.UNSIGNED_INT;if(i===ri)return n.FLOAT;if(i===si)return n.HALF_FLOAT;if(i===Xc)return n.ALPHA;if(i===qc)return n.RGB;if(i===$n)return n.RGBA;if(i===di)return n.DEPTH_COMPONENT;if(i===ji)return n.DEPTH_STENCIL;if(i===$c)return n.RED;if(i===Fa)return n.RED_INTEGER;if(i===Qi)return n.RG;if(i===Oa)return n.RG_INTEGER;if(i===Ba)return n.RGBA_INTEGER;if(i===io||i===ro||i===so||i===oo)if(o===Mt)if(s=e.get("WEBGL_compressed_texture_s3tc_srgb"),s!==null){if(i===io)return s.COMPRESSED_SRGB_S3TC_DXT1_EXT;if(i===ro)return s.COMPRESSED_SRGB_ALPHA_S3TC_DXT1_EXT;if(i===so)return s.COMPRESSED_SRGB_ALPHA_S3TC_DXT3_EXT;if(i===oo)return s.COMPRESSED_SRGB_ALPHA_S3TC_DXT5_EXT}else return null;else if(s=e.get("WEBGL_compressed_texture_s3tc"),s!==null){if(i===io)return s.COMPRESSED_RGB_S3TC_DXT1_EXT;if(i===ro)return s.COMPRESSED_RGBA_S3TC_DXT1_EXT;if(i===so)return s.COMPRESSED_RGBA_S3TC_DXT3_EXT;if(i===oo)return s.COMPRESSED_RGBA_S3TC_DXT5_EXT}else return null;if(i===za||i===Ha||i===Va||i===Ga)if(s=e.get("WEBGL_compressed_texture_pvrtc"),s!==null){if(i===za)return s.COMPRESSED_RGB_PVRTC_4BPPV1_IMG;if(i===Ha)return s.COMPRESSED_RGB_PVRTC_2BPPV1_IMG;if(i===Va)return s.COMPRESSED_RGBA_PVRTC_4BPPV1_IMG;if(i===Ga)return s.COMPRESSED_RGBA_PVRTC_2BPPV1_IMG}else return null;if(i===Wa||i===Xa||i===qa||i===$a||i===Ya||i===ao||i===Za)if(s=e.get("WEBGL_compressed_texture_etc"),s!==null){if(i===Wa||i===Xa)return o===Mt?s.COMPRESSED_SRGB8_ETC2:s.COMPRESSED_RGB8_ETC2;if(i===qa)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ETC2_EAC:s.COMPRESSED_RGBA8_ETC2_EAC;if(i===$a)return s.COMPRESSED_R11_EAC;if(i===Ya)return s.COMPRESSED_SIGNED_R11_EAC;if(i===ao)return s.COMPRESSED_RG11_EAC;if(i===Za)return s.COMPRESSED_SIGNED_RG11_EAC}else return null;if(i===Ka||i===Ja||i===ja||i===Qa||i===el||i===tl||i===nl||i===il||i===rl||i===sl||i===ol||i===al||i===ll||i===cl)if(s=e.get("WEBGL_compressed_texture_astc"),s!==null){if(i===Ka)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_4x4_KHR:s.COMPRESSED_RGBA_ASTC_4x4_KHR;if(i===Ja)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_5x4_KHR:s.COMPRESSED_RGBA_ASTC_5x4_KHR;if(i===ja)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_5x5_KHR:s.COMPRESSED_RGBA_ASTC_5x5_KHR;if(i===Qa)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_6x5_KHR:s.COMPRESSED_RGBA_ASTC_6x5_KHR;if(i===el)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_6x6_KHR:s.COMPRESSED_RGBA_ASTC_6x6_KHR;if(i===tl)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_8x5_KHR:s.COMPRESSED_RGBA_ASTC_8x5_KHR;if(i===nl)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_8x6_KHR:s.COMPRESSED_RGBA_ASTC_8x6_KHR;if(i===il)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_8x8_KHR:s.COMPRESSED_RGBA_ASTC_8x8_KHR;if(i===rl)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x5_KHR:s.COMPRESSED_RGBA_ASTC_10x5_KHR;if(i===sl)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x6_KHR:s.COMPRESSED_RGBA_ASTC_10x6_KHR;if(i===ol)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x8_KHR:s.COMPRESSED_RGBA_ASTC_10x8_KHR;if(i===al)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x10_KHR:s.COMPRESSED_RGBA_ASTC_10x10_KHR;if(i===ll)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_12x10_KHR:s.COMPRESSED_RGBA_ASTC_12x10_KHR;if(i===cl)return o===Mt?s.COMPRESSED_SRGB8_ALPHA8_ASTC_12x12_KHR:s.COMPRESSED_RGBA_ASTC_12x12_KHR}else return null;if(i===ul||i===hl||i===dl)if(s=e.get("EXT_texture_compression_bptc"),s!==null){if(i===ul)return o===Mt?s.COMPRESSED_SRGB_ALPHA_BPTC_UNORM_EXT:s.COMPRESSED_RGBA_BPTC_UNORM_EXT;if(i===hl)return s.COMPRESSED_RGB_BPTC_SIGNED_FLOAT_EXT;if(i===dl)return s.COMPRESSED_RGB_BPTC_UNSIGNED_FLOAT_EXT}else return null;if(i===fl||i===pl||i===lo||i===ml)if(s=e.get("EXT_texture_compression_rgtc"),s!==null){if(i===fl)return s.COMPRESSED_RED_RGTC1_EXT;if(i===pl)return s.COMPRESSED_SIGNED_RED_RGTC1_EXT;if(i===lo)return s.COMPRESSED_RED_GREEN_RGTC2_EXT;if(i===ml)return s.COMPRESSED_SIGNED_RED_GREEN_RGTC2_EXT}else return null;return i===us?n.UNSIGNED_INT_24_8:n[i]!==void 0?n[i]:null}return{convert:t}}var qv=`
void main() {

	gl_Position = vec4( position, 1.0 );

}`,$v=`
uniform sampler2DArray depthColor;
uniform float depthWidth;
uniform float depthHeight;

void main() {

	vec2 coord = vec2( gl_FragCoord.x / depthWidth, gl_FragCoord.y / depthHeight );

	if ( coord.x >= 1.0 ) {

		gl_FragDepth = texture( depthColor, vec3( coord.x - 1.0, coord.y, 1 ) ).r;

	} else {

		gl_FragDepth = texture( depthColor, vec3( coord.x, coord.y, 0 ) ).r;

	}

}`,mu=class{constructor(){this.texture=null,this.mesh=null,this.depthNear=0,this.depthFar=0}init(e,t){if(this.texture===null){let i=new Hs(e.texture);(e.depthNear!==t.depthNear||e.depthFar!==t.depthFar)&&(this.depthNear=e.depthNear,this.depthFar=e.depthFar),this.texture=i}}getMesh(e){if(this.texture!==null&&this.mesh===null){let t=e.cameras[0].viewport,i=new On({vertexShader:qv,fragmentShader:$v,uniforms:{depthColor:{value:this.texture},depthWidth:{value:t.z},depthHeight:{value:t.w}}});this.mesh=new xt(new Gi(20,20),i)}return this.mesh}reset(){this.texture=null,this.mesh=null}getDepthTexture(){return this.texture}},gu=class extends ti{constructor(e,t){super();let i=this,r=null,s=1,o=null,a="local-floor",c=1,l=null,h=null,m=null,u=null,g=null,_=null,x=typeof XRWebGLBinding<"u",f=new mu,d={},S=t.getContextAttributes(),P=null,w=null,T=[],A=[],D=new He,p=null,M=null,y=new mn;y.viewport=new Ot;let C=new mn;C.viewport=new Ot;let k=[y,C],L=new Ca,U=null,F=null;this.cameraAutoUpdate=!0,this.enabled=!1,this.isPresenting=!1,this.getController=function(se){let le=T[se];return le===void 0&&(le=new jr,T[se]=le),le.getTargetRaySpace()},this.getControllerGrip=function(se){let le=T[se];return le===void 0&&(le=new jr,T[se]=le),le.getGripSpace()},this.getHand=function(se){let le=T[se];return le===void 0&&(le=new jr,T[se]=le),le.getHandSpace()};function H(se){let le=A.indexOf(se.inputSource);if(le===-1)return;let Se=T[le];Se!==void 0&&(Se.update(se.inputSource,se.frame,l||o),Se.dispatchEvent({type:se.type,data:se.inputSource}))}function q(){r.removeEventListener("select",H),r.removeEventListener("selectstart",H),r.removeEventListener("selectend",H),r.removeEventListener("squeeze",H),r.removeEventListener("squeezestart",H),r.removeEventListener("squeezeend",H),r.removeEventListener("end",q),r.removeEventListener("inputsourceschange",ee);for(let se=0;se<T.length;se++){let le=A[se];le!==null&&(A[se]=null,T[se].disconnect(le))}U=null,F=null,f.reset();for(let se in d)delete d[se];if(e.setRenderTarget(P),g=null,u=null,m=null,r=null,w=null,Je.stop(),i.isPresenting=!1,e.setPixelRatio(p),e.setSize(D.width,D.height,!1),M!==null){let se=M.camera;se.fov=M.fov,se.zoom=M.zoom,se.updateProjectionMatrix(),M=null}i.dispatchEvent({type:"sessionend"})}this.setFramebufferScaleFactor=function(se){s=se,i.isPresenting===!0&&Ge("WebXRManager: Cannot change framebuffer scale while presenting.")},this.setReferenceSpaceType=function(se){a=se,i.isPresenting===!0&&Ge("WebXRManager: Cannot change reference space type while presenting.")},this.getReferenceSpace=function(){return l||o},this.setReferenceSpace=function(se){l=se},this.getBaseLayer=function(){return u!==null?u:g},this.getBinding=function(){return m===null&&x&&(m=new XRWebGLBinding(r,t)),m},this.getFrame=function(){return _},this.getSession=function(){return r},this.setSession=async function(se){if(r=se,r!==null){if(P=e.getRenderTarget(),r.addEventListener("select",H),r.addEventListener("selectstart",H),r.addEventListener("selectend",H),r.addEventListener("squeeze",H),r.addEventListener("squeezestart",H),r.addEventListener("squeezeend",H),r.addEventListener("end",q),r.addEventListener("inputsourceschange",ee),S.xrCompatible!==!0&&await t.makeXRCompatible(),p=e.getPixelRatio(),e.getSize(D),x&&"createProjectionLayer"in XRWebGLBinding.prototype){let Se=null,ke=null,ye=null;S.depth&&(ye=S.stencil?t.DEPTH24_STENCIL8:t.DEPTH_COMPONENT24,Se=S.stencil?ji:di,ke=S.stencil?us:ii);let je={colorFormat:t.RGBA8,depthFormat:ye,scaleFactor:s};m=this.getBinding(),u=m.createProjectionLayer(je),r.updateRenderState({layers:[u]}),e.setPixelRatio(1),e.setSize(u.textureWidth,u.textureHeight,!1),w=new In(u.textureWidth,u.textureHeight,{format:$n,type:Ln,depthTexture:new Vi(u.textureWidth,u.textureHeight,ke,void 0,void 0,void 0,void 0,void 0,void 0,Se),stencilBuffer:S.stencil,colorSpace:e.outputColorSpace,samples:S.antialias?4:0,resolveDepthBuffer:u.ignoreDepthValues===!1,resolveStencilBuffer:u.ignoreDepthValues===!1,storeMultisampledDepthBuffer:u.ignoreDepthValues===!1,storeMultisampledStencilBuffer:u.ignoreDepthValues===!1})}else{let Se={antialias:S.antialias,alpha:!0,depth:S.depth,stencil:S.stencil,framebufferScaleFactor:s};g=new XRWebGLLayer(r,t,Se),r.updateRenderState({baseLayer:g}),e.setPixelRatio(1),e.setSize(g.framebufferWidth,g.framebufferHeight,!1),w=new In(g.framebufferWidth,g.framebufferHeight,{format:$n,type:Ln,colorSpace:e.outputColorSpace,stencilBuffer:S.stencil,resolveDepthBuffer:g.ignoreDepthValues===!1,resolveStencilBuffer:g.ignoreDepthValues===!1,storeMultisampledDepthBuffer:g.ignoreDepthValues===!1,storeMultisampledStencilBuffer:g.ignoreDepthValues===!1})}w.isXRRenderTarget=!0,this.setFoveation(c),l=null,o=await r.requestReferenceSpace(a),Je.setContext(r),Je.start(),i.isPresenting=!0,i.dispatchEvent({type:"sessionstart"})}},this.getEnvironmentBlendMode=function(){if(r!==null)return r.environmentBlendMode},this.getDepthTexture=function(){return f.getDepthTexture()};function ee(se){for(let le=0;le<se.removed.length;le++){let Se=se.removed[le],ke=A.indexOf(Se);ke>=0&&(A[ke]=null,T[ke].disconnect(Se))}for(let le=0;le<se.added.length;le++){let Se=se.added[le],ke=A.indexOf(Se);if(ke===-1){for(let je=0;je<T.length;je++)if(je>=A.length){A.push(Se),ke=je;break}else if(A[je]===null){A[je]=Se,ke=je;break}if(ke===-1)break}let ye=T[ke];ye&&ye.connect(Se)}}let J=new V,ne=new V;function ce(se,le,Se){J.setFromMatrixPosition(le.matrixWorld),ne.setFromMatrixPosition(Se.matrixWorld);let ke=J.distanceTo(ne),ye=le.projectionMatrix.elements,je=Se.projectionMatrix.elements,Nt=ye[14]/(ye[10]-1),Ye=ye[14]/(ye[10]+1),nt=(ye[9]+1)/ye[5],ft=(ye[9]-1)/ye[5],Qe=(ye[8]-1)/ye[0],Tt=(je[8]+1)/je[0],Bt=Nt*Qe,$t=Nt*Tt,wt=ke/(-Qe+Tt),Ut=wt*-Qe;if(le.matrixWorld.decompose(se.position,se.quaternion,se.scale),se.translateX(Ut),se.translateZ(wt),se.matrixWorld.compose(se.position,se.quaternion,se.scale),se.matrixWorldInverse.copy(se.matrixWorld).invert(),ye[10]===-1)se.projectionMatrix.copy(le.projectionMatrix),se.projectionMatrixInverse.copy(le.projectionMatrixInverse);else{let W=Nt+wt,Qt=Ye+wt,at=Bt-Ut,N=$t+(ke-Ut),b=nt*Ye/Qt*W,Z=ft*Ye/Qt*W;se.projectionMatrix.makePerspective(at,N,b,Z,W,Qt),se.projectionMatrixInverse.copy(se.projectionMatrix).invert()}}function pe(se,le){le===null?se.matrixWorld.copy(se.matrix):se.matrixWorld.multiplyMatrices(le.matrixWorld,se.matrix),se.matrixWorldInverse.copy(se.matrixWorld).invert()}this.updateCamera=function(se){if(r===null)return;let le=se.near,Se=se.far;f.texture!==null&&(f.depthNear>0&&(le=f.depthNear),f.depthFar>0&&(Se=f.depthFar)),L.near=C.near=y.near=le,L.far=C.far=y.far=Se,(U!==L.near||F!==L.far)&&(r.updateRenderState({depthNear:L.near,depthFar:L.far}),U=L.near,F=L.far),L.layers.mask=se.layers.mask|6,y.layers.mask=L.layers.mask&-5,C.layers.mask=L.layers.mask&-3;let ke=se.parent,ye=L.cameras;pe(L,ke);for(let je=0;je<ye.length;je++)pe(ye[je],ke);ye.length===2?ce(L,y,C):L.projectionMatrix.copy(y.projectionMatrix),M===null&&se.isPerspectiveCamera&&(M={camera:se,fov:se.fov,zoom:se.zoom}),me(se,L,ke)};function me(se,le,Se){Se===null?se.matrix.copy(le.matrixWorld):(se.matrix.copy(Se.matrixWorld),se.matrix.invert(),se.matrix.multiply(le.matrixWorld)),se.matrix.decompose(se.position,se.quaternion,se.scale),se.updateMatrixWorld(!0),se.projectionMatrix.copy(le.projectionMatrix),se.projectionMatrixInverse.copy(le.projectionMatrixInverse),se.isPerspectiveCamera&&(se.fov=Zr*2*Math.atan(1/se.projectionMatrix.elements[5]),se.zoom=1)}this.getCamera=function(){return L},this.getFoveation=function(){if(!(u===null&&g===null))return c},this.setFoveation=function(se){c=se,u!==null&&(u.fixedFoveation=se),g!==null&&g.fixedFoveation!==void 0&&(g.fixedFoveation=se)},this.hasDepthSensing=function(){return f.texture!==null},this.getDepthSensingMesh=function(){return f.getMesh(L)},this.getCameraTexture=function(se){return d[se]};let qe=null;function Le(se,le){if(h=le.getViewerPose(l||o),_=le,h!==null){let Se=h.views;g!==null&&(e.setRenderTargetFramebuffer(w,g.framebuffer),e.setRenderTarget(w));let ke=!1;Se.length!==L.cameras.length&&(L.cameras.length=0,ke=!0);for(let Ye=0;Ye<Se.length;Ye++){let nt=Se[Ye],ft=null;if(g!==null)ft=g.getViewport(nt);else{let Tt=m.getViewSubImage(u,nt);ft=Tt.viewport,Ye===0&&(e.setRenderTargetTextures(w,Tt.colorTexture,Tt.depthStencilTexture),e.setRenderTarget(w))}let Qe=k[Ye];Qe===void 0&&(Qe=new mn,Qe.layers.enable(Ye),Qe.viewport=new Ot,k[Ye]=Qe),Qe.matrix.fromArray(nt.transform.matrix),Qe.matrix.decompose(Qe.position,Qe.quaternion,Qe.scale),Qe.projectionMatrix.fromArray(nt.projectionMatrix),Qe.projectionMatrixInverse.copy(Qe.projectionMatrix).invert(),Qe.viewport.set(ft.x,ft.y,ft.width,ft.height),Ye===0&&(L.matrix.copy(Qe.matrix),L.matrix.decompose(L.position,L.quaternion,L.scale)),ke===!0&&L.cameras.push(Qe)}let ye=r.enabledFeatures;if(ye&&ye.includes("depth-sensing")&&r.depthUsage=="gpu-optimized"&&x){m=i.getBinding();let Ye=m.getDepthInformation(Se[0]);Ye&&Ye.isValid&&Ye.texture&&f.init(Ye,r.renderState)}if(ye&&ye.includes("camera-access")&&x){e.state.unbindTexture(),m=i.getBinding();for(let Ye=0;Ye<Se.length;Ye++){let nt=Se[Ye].camera;if(nt){let ft=d[nt];ft||(ft=new Hs,d[nt]=ft);let Qe=m.getCameraImage(nt);ft.sourceTexture=Qe}}}}for(let Se=0;Se<T.length;Se++){let ke=A[Se],ye=T[Se];ke!==null&&ye!==void 0&&ye.update(ke,le,l||o)}qe&&qe(se,le),le.detectedPlanes&&i.dispatchEvent({type:"planesdetected",data:le}),_=null}let Je=new zd;Je.setAnimationLoop(Le),this.setAnimationLoop=function(se){qe=se},this.dispose=function(){}}},Yv=new It,qd=new Ze;qd.set(-1,0,0,0,1,0,0,0,1);function Zv(n,e){function t(f,d){f.matrixAutoUpdate===!0&&f.updateMatrix(),d.value.copy(f.matrix)}function i(f,d){d.color.getRGB(f.fogColor.value,jc(n)),d.isFog?(f.fogNear.value=d.near,f.fogFar.value=d.far):d.isFogExp2&&(f.fogDensity.value=d.density)}function r(f,d,S,P,w){d.isNodeMaterial?d.uniformsNeedUpdate=!1:d.isMeshBasicMaterial?s(f,d):d.isMeshLambertMaterial?(s(f,d),d.envMap&&(f.envMapIntensity.value=d.envMapIntensity)):d.isMeshToonMaterial?(s(f,d),m(f,d)):d.isMeshPhongMaterial?(s(f,d),h(f,d),d.envMap&&(f.envMapIntensity.value=d.envMapIntensity)):d.isMeshStandardMaterial?(s(f,d),u(f,d),d.isMeshPhysicalMaterial&&g(f,d,w)):d.isMeshMatcapMaterial?(s(f,d),_(f,d)):d.isMeshDepthMaterial?s(f,d):d.isMeshDistanceMaterial?(s(f,d),x(f,d)):d.isMeshNormalMaterial?s(f,d):d.isLineBasicMaterial?(o(f,d),d.isLineDashedMaterial&&a(f,d)):d.isPointsMaterial?c(f,d,S,P):d.isSpriteMaterial?l(f,d):d.isShadowMaterial?(f.color.value.copy(d.color),f.opacity.value=d.opacity):d.isShaderMaterial&&(d.uniformsNeedUpdate=!1)}function s(f,d){f.opacity.value=d.opacity,d.color&&f.diffuse.value.copy(d.color),d.emissive&&f.emissive.value.copy(d.emissive).multiplyScalar(d.emissiveIntensity),d.map&&(f.map.value=d.map,t(d.map,f.mapTransform)),d.alphaMap&&(f.alphaMap.value=d.alphaMap,t(d.alphaMap,f.alphaMapTransform)),d.bumpMap&&(f.bumpMap.value=d.bumpMap,t(d.bumpMap,f.bumpMapTransform),f.bumpScale.value=d.bumpScale,d.side===Rn&&(f.bumpScale.value*=-1)),d.normalMap&&(f.normalMap.value=d.normalMap,t(d.normalMap,f.normalMapTransform),f.normalScale.value.copy(d.normalScale),d.side===Rn&&f.normalScale.value.negate()),d.displacementMap&&(f.displacementMap.value=d.displacementMap,t(d.displacementMap,f.displacementMapTransform),f.displacementScale.value=d.displacementScale,f.displacementBias.value=d.displacementBias),d.emissiveMap&&(f.emissiveMap.value=d.emissiveMap,t(d.emissiveMap,f.emissiveMapTransform)),d.specularMap&&(f.specularMap.value=d.specularMap,t(d.specularMap,f.specularMapTransform)),d.alphaTest>0&&(f.alphaTest.value=d.alphaTest);let S=e.get(d),P=S.envMap,w=S.envMapRotation;P&&(f.envMap.value=P,f.envMapRotation.value.setFromMatrix4(Yv.makeRotationFromEuler(w)).transpose(),P.isCubeTexture&&P.isRenderTargetTexture===!1&&f.envMapRotation.value.premultiply(qd),f.reflectivity.value=d.reflectivity,f.ior.value=d.ior,f.refractionRatio.value=d.refractionRatio),d.lightMap&&(f.lightMap.value=d.lightMap,f.lightMapIntensity.value=d.lightMapIntensity,t(d.lightMap,f.lightMapTransform)),d.aoMap&&(f.aoMap.value=d.aoMap,f.aoMapIntensity.value=d.aoMapIntensity,t(d.aoMap,f.aoMapTransform))}function o(f,d){f.diffuse.value.copy(d.color),f.opacity.value=d.opacity,d.map&&(f.map.value=d.map,t(d.map,f.mapTransform))}function a(f,d){f.dashSize.value=d.dashSize,f.totalSize.value=d.dashSize+d.gapSize,f.scale.value=d.scale}function c(f,d,S,P){f.diffuse.value.copy(d.color),f.opacity.value=d.opacity,f.size.value=d.size*S,f.scale.value=P*.5,d.map&&(f.map.value=d.map,t(d.map,f.uvTransform)),d.alphaMap&&(f.alphaMap.value=d.alphaMap,t(d.alphaMap,f.alphaMapTransform)),d.alphaTest>0&&(f.alphaTest.value=d.alphaTest)}function l(f,d){f.diffuse.value.copy(d.color),f.opacity.value=d.opacity,f.rotation.value=d.rotation,d.map&&(f.map.value=d.map,t(d.map,f.mapTransform)),d.alphaMap&&(f.alphaMap.value=d.alphaMap,t(d.alphaMap,f.alphaMapTransform)),d.alphaTest>0&&(f.alphaTest.value=d.alphaTest)}function h(f,d){f.specular.value.copy(d.specular),f.shininess.value=Math.max(d.shininess,1e-4)}function m(f,d){d.gradientMap&&(f.gradientMap.value=d.gradientMap)}function u(f,d){f.metalness.value=d.metalness,d.metalnessMap&&(f.metalnessMap.value=d.metalnessMap,t(d.metalnessMap,f.metalnessMapTransform)),f.roughness.value=d.roughness,d.roughnessMap&&(f.roughnessMap.value=d.roughnessMap,t(d.roughnessMap,f.roughnessMapTransform)),d.envMap&&(f.envMapIntensity.value=d.envMapIntensity)}function g(f,d,S){f.ior.value=d.ior,d.sheen>0&&(f.sheenColor.value.copy(d.sheenColor).multiplyScalar(d.sheen),f.sheenRoughness.value=d.sheenRoughness,d.sheenColorMap&&(f.sheenColorMap.value=d.sheenColorMap,t(d.sheenColorMap,f.sheenColorMapTransform)),d.sheenRoughnessMap&&(f.sheenRoughnessMap.value=d.sheenRoughnessMap,t(d.sheenRoughnessMap,f.sheenRoughnessMapTransform))),d.clearcoat>0&&(f.clearcoat.value=d.clearcoat,f.clearcoatRoughness.value=d.clearcoatRoughness,d.clearcoatMap&&(f.clearcoatMap.value=d.clearcoatMap,t(d.clearcoatMap,f.clearcoatMapTransform)),d.clearcoatRoughnessMap&&(f.clearcoatRoughnessMap.value=d.clearcoatRoughnessMap,t(d.clearcoatRoughnessMap,f.clearcoatRoughnessMapTransform)),d.clearcoatNormalMap&&(f.clearcoatNormalMap.value=d.clearcoatNormalMap,t(d.clearcoatNormalMap,f.clearcoatNormalMapTransform),f.clearcoatNormalScale.value.copy(d.clearcoatNormalScale),d.side===Rn&&f.clearcoatNormalScale.value.negate())),d.dispersion>0&&(f.dispersion.value=d.dispersion),d.retroreflectivity>0&&(f.retroreflectivity.value=d.retroreflectivity),d.iridescence>0&&(f.iridescence.value=d.iridescence,f.iridescenceIOR.value=d.iridescenceIOR,f.iridescenceThicknessMinimum.value=d.iridescenceThicknessRange[0],f.iridescenceThicknessMaximum.value=d.iridescenceThicknessRange[1],d.iridescenceMap&&(f.iridescenceMap.value=d.iridescenceMap,t(d.iridescenceMap,f.iridescenceMapTransform)),d.iridescenceThicknessMap&&(f.iridescenceThicknessMap.value=d.iridescenceThicknessMap,t(d.iridescenceThicknessMap,f.iridescenceThicknessMapTransform))),d.transmission>0&&(f.transmission.value=d.transmission,f.transmissionSamplerMap.value=S.texture,f.transmissionSamplerSize.value.set(S.width,S.height),d.transmissionMap&&(f.transmissionMap.value=d.transmissionMap,t(d.transmissionMap,f.transmissionMapTransform)),f.thickness.value=d.thickness,d.thicknessMap&&(f.thicknessMap.value=d.thicknessMap,t(d.thicknessMap,f.thicknessMapTransform)),f.attenuationDistance.value=d.attenuationDistance,f.attenuationColor.value.copy(d.attenuationColor)),d.anisotropy>0&&(f.anisotropyVector.value.set(d.anisotropy*Math.cos(d.anisotropyRotation),d.anisotropy*Math.sin(d.anisotropyRotation)),d.anisotropyMap&&(f.anisotropyMap.value=d.anisotropyMap,t(d.anisotropyMap,f.anisotropyMapTransform))),f.specularIntensity.value=d.specularIntensity,f.specularColor.value.copy(d.specularColor),d.specularColorMap&&(f.specularColorMap.value=d.specularColorMap,t(d.specularColorMap,f.specularColorMapTransform)),d.specularIntensityMap&&(f.specularIntensityMap.value=d.specularIntensityMap,t(d.specularIntensityMap,f.specularIntensityMapTransform))}function _(f,d){d.matcap&&(f.matcap.value=d.matcap)}function x(f,d){let S=e.get(d).light;f.referencePosition.value.setFromMatrixPosition(S.matrixWorld),f.nearDistance.value=S.shadow.camera.near,f.farDistance.value=S.shadow.camera.far}return{refreshFogUniforms:i,refreshMaterialUniforms:r}}function Kv(n,e,t,i){let r={},s={},o=[],a=n.getParameter(n.MAX_UNIFORM_BUFFER_BINDINGS);function c(w,T){let A=T.program;i.uniformBlockBinding(w,A)}function l(w,T){let A=r[w.id];A===void 0&&(f(w),A=h(w),r[w.id]=A,w.addEventListener("dispose",S));let D=T.program;i.updateUBOMapping(w,D);let p=e.render.frame;s[w.id]!==p&&(u(w),s[w.id]=p)}function h(w){let T=m();w.__bindingPointIndex=T;let A=n.createBuffer(),D=w.__size,p=w.usage;return n.bindBuffer(n.UNIFORM_BUFFER,A),n.bufferData(n.UNIFORM_BUFFER,D,p),n.bindBuffer(n.UNIFORM_BUFFER,null),n.bindBufferBase(n.UNIFORM_BUFFER,T,A),A}function m(){for(let w=0;w<a;w++)if(o.indexOf(w)===-1)return o.push(w),w;return Xe("WebGLRenderer: Maximum number of simultaneously usable uniforms groups reached."),0}function u(w){let T=r[w.id],A=w.uniforms,D=w.__cache;n.bindBuffer(n.UNIFORM_BUFFER,T);for(let p=0,M=A.length;p<M;p++){let y=A[p];if(Array.isArray(y))for(let C=0,k=y.length;C<k;C++)g(y[C],p,C,D);else g(y,p,0,D)}n.bindBuffer(n.UNIFORM_BUFFER,null)}function g(w,T,A,D){if(x(w,T,A,D)===!0){let p=w.__offset,M=w.value;if(Array.isArray(M)){let y=0;for(let C=0;C<M.length;C++){let k=M[C],L=d(k);_(k,w.__data,y),typeof k!="number"&&typeof k!="boolean"&&!k.isMatrix3&&!ArrayBuffer.isView(k)&&(y+=L.storage/Float32Array.BYTES_PER_ELEMENT)}}else _(M,w.__data,0);n.bufferSubData(n.UNIFORM_BUFFER,p,w.__data)}}function _(w,T,A){typeof w=="number"||typeof w=="boolean"?T[0]=w:w.isMatrix3?(T[0]=w.elements[0],T[1]=w.elements[1],T[2]=w.elements[2],T[3]=0,T[4]=w.elements[3],T[5]=w.elements[4],T[6]=w.elements[5],T[7]=0,T[8]=w.elements[6],T[9]=w.elements[7],T[10]=w.elements[8],T[11]=0):ArrayBuffer.isView(w)?T.set(new w.constructor(w.buffer,w.byteOffset,T.length)):w.toArray(T,A)}function x(w,T,A,D){let p=w.value,M=T+"_"+A;if(D[M]===void 0)return typeof p=="number"||typeof p=="boolean"?D[M]=p:ArrayBuffer.isView(p)?D[M]=p.slice():D[M]=p.clone(),!0;{let y=D[M];if(typeof p=="number"||typeof p=="boolean"){if(y!==p)return D[M]=p,!0}else{if(ArrayBuffer.isView(p))return!0;if(y.equals(p)===!1)return y.copy(p),!0}}return!1}function f(w){let T=w.uniforms,A=0,D=16;for(let M=0,y=T.length;M<y;M++){let C=Array.isArray(T[M])?T[M]:[T[M]];for(let k=0,L=C.length;k<L;k++){let U=C[k],F=Array.isArray(U.value)?U.value:[U.value];for(let H=0,q=F.length;H<q;H++){let ee=F[H],J=d(ee),ne=A%D,ce=ne%J.boundary,pe=ne+ce;A+=ce,pe!==0&&D-pe<J.storage&&(A+=D-pe),U.__data=new Float32Array(J.storage/Float32Array.BYTES_PER_ELEMENT),U.__offset=A,A+=J.storage}}}let p=A%D;return p>0&&(A+=D-p),w.__size=A,w.__cache={},this}function d(w){let T={boundary:0,storage:0};return typeof w=="number"||typeof w=="boolean"?(T.boundary=4,T.storage=4):w.isVector2?(T.boundary=8,T.storage=8):w.isVector3||w.isColor?(T.boundary=16,T.storage=12):w.isVector4?(T.boundary=16,T.storage=16):w.isMatrix3?(T.boundary=48,T.storage=48):w.isMatrix4?(T.boundary=64,T.storage=64):w.isTexture?Ge("WebGLRenderer: Texture samplers can not be part of an uniforms group."):ArrayBuffer.isView(w)?(T.boundary=16,T.storage=w.byteLength):Ge("WebGLRenderer: Unsupported uniform value type.",w),T}function S(w){let T=w.target;T.removeEventListener("dispose",S);let A=o.indexOf(T.__bindingPointIndex);o.splice(A,1),n.deleteBuffer(r[T.id]),delete r[T.id],delete s[T.id]}function P(){for(let w in r)n.deleteBuffer(r[w]);o=[],r={},s={}}return{bind:c,update:l,dispose:P}}var Jv=new Uint16Array([12469,15057,12620,14925,13266,14620,13807,14376,14323,13990,14545,13625,14713,13328,14840,12882,14931,12528,14996,12233,15039,11829,15066,11525,15080,11295,15085,10976,15082,10705,15073,10495,13880,14564,13898,14542,13977,14430,14158,14124,14393,13732,14556,13410,14702,12996,14814,12596,14891,12291,14937,11834,14957,11489,14958,11194,14943,10803,14921,10506,14893,10278,14858,9960,14484,14039,14487,14025,14499,13941,14524,13740,14574,13468,14654,13106,14743,12678,14818,12344,14867,11893,14889,11509,14893,11180,14881,10751,14852,10428,14812,10128,14765,9754,14712,9466,14764,13480,14764,13475,14766,13440,14766,13347,14769,13070,14786,12713,14816,12387,14844,11957,14860,11549,14868,11215,14855,10751,14825,10403,14782,10044,14729,9651,14666,9352,14599,9029,14967,12835,14966,12831,14963,12804,14954,12723,14936,12564,14917,12347,14900,11958,14886,11569,14878,11247,14859,10765,14828,10401,14784,10011,14727,9600,14660,9289,14586,8893,14508,8533,15111,12234,15110,12234,15104,12216,15092,12156,15067,12010,15028,11776,14981,11500,14942,11205,14902,10752,14861,10393,14812,9991,14752,9570,14682,9252,14603,8808,14519,8445,14431,8145,15209,11449,15208,11451,15202,11451,15190,11438,15163,11384,15117,11274,15055,10979,14994,10648,14932,10343,14871,9936,14803,9532,14729,9218,14645,8742,14556,8381,14461,8020,14365,7603,15273,10603,15272,10607,15267,10619,15256,10631,15231,10614,15182,10535,15118,10389,15042,10167,14963,9787,14883,9447,14800,9115,14710,8665,14615,8318,14514,7911,14411,7507,14279,7198,15314,9675,15313,9683,15309,9712,15298,9759,15277,9797,15229,9773,15166,9668,15084,9487,14995,9274,14898,8910,14800,8539,14697,8234,14590,7790,14479,7409,14367,7067,14178,6621,15337,8619,15337,8631,15333,8677,15325,8769,15305,8871,15264,8940,15202,8909,15119,8775,15022,8565,14916,8328,14804,8009,14688,7614,14569,7287,14448,6888,14321,6483,14088,6171,15350,7402,15350,7419,15347,7480,15340,7613,15322,7804,15287,7973,15229,8057,15148,8012,15046,7846,14933,7611,14810,7357,14682,7069,14552,6656,14421,6316,14251,5948,14007,5528,15356,5942,15356,5977,15353,6119,15348,6294,15332,6551,15302,6824,15249,7044,15171,7122,15070,7050,14949,6861,14818,6611,14679,6349,14538,6067,14398,5651,14189,5311,13935,4958,15359,4123,15359,4153,15356,4296,15353,4646,15338,5160,15311,5508,15263,5829,15188,6042,15088,6094,14966,6001,14826,5796,14678,5543,14527,5287,14377,4985,14133,4586,13869,4257,15360,1563,15360,1642,15358,2076,15354,2636,15341,3350,15317,4019,15273,4429,15203,4732,15105,4911,14981,4932,14836,4818,14679,4621,14517,4386,14359,4156,14083,3795,13808,3437,15360,122,15360,137,15358,285,15355,636,15344,1274,15322,2177,15281,2765,15215,3223,15120,3451,14995,3569,14846,3567,14681,3466,14511,3305,14344,3121,14037,2800,13753,2467,15360,0,15360,1,15359,21,15355,89,15346,253,15325,479,15287,796,15225,1148,15133,1492,15008,1749,14856,1882,14685,1886,14506,1783,14324,1608,13996,1398,13702,1183]),gi=null;function jv(){return gi===null&&(gi=new ca(Jv,16,16,Qi,si),gi.name="DFG_LUT",gi.minFilter=gn,gi.magFilter=gn,gi.wrapS=hi,gi.wrapT=hi,gi.generateMipmaps=!1,gi.needsUpdate=!0),gi}var Sl=class{constructor(e={}){let{canvas:t=hd(),context:i=null,depth:r=!0,stencil:s=!1,alpha:o=!1,antialias:a=!1,premultipliedAlpha:c=!0,preserveDrawingBuffer:l=!1,powerPreference:h="default",failIfMajorPerformanceCaveat:m=!1,reversedDepthBuffer:u=!1,outputBufferType:g=Ln}=e;this.isWebGLRenderer=!0;let _;if(i!==null){if(typeof WebGLRenderingContext<"u"&&i instanceof WebGLRenderingContext)throw new Error("THREE.WebGLRenderer: WebGL 1 is not supported since r163.");_=i.getContextAttributes().alpha}else _=o;let x=g,f=new Set([Ba,Oa,Fa]),d=new Set([Ln,ii,cs,us,Na,Ua]),S=new Uint32Array(4),P=new Int32Array(4),w=new V,T=null,A=null,D=[],p=[],M=null;this.domElement=t,this.debug={checkShaderErrors:!0,diagnostics:{keywords:!1},onShaderError:null},this.autoClear=!0,this.autoClearColor=!0,this.autoClearDepth=!0,this.autoClearStencil=!0,this.sortObjects=!0,this.clippingPlanes=[],this.localClippingEnabled=!1,this.toneMapping=ni,this.toneMappingExposure=1,this.transmissionResolutionScale=1;let y=this,C=!1,k=null,L=null,U=null,F=null;this._outputColorSpace=Ht;let H=0,q=0,ee=null,J=-1,ne=null,ce=new Ot,pe=new Ot,me=null,qe=new ze(0),Le=0,Je=t.width,se=t.height,le=1,Se=null,ke=null,ye=new Ot(0,0,Je,se),je=new Ot(0,0,Je,se),Nt=!1,Ye=new Qr,nt=!1,ft=!1,Qe=new It,Tt=new V,Bt=new Ot,$t={background:null,fog:null,environment:null,overrideMaterial:null,isScene:!0},wt=!1;function Ut(){return ee===null?le:1}let W=i;function Qt(E,G){return t.getContext(E,G)}let at,N,b,Z,te,ae,ve,Me,R,I,O,z,X,Y,re,fe,ge,B,ue,oe,xe,_e,he;try{let E={alpha:!0,depth:r,stencil:s,antialias:a,premultipliedAlpha:c,preserveDrawingBuffer:l,powerPreference:h,failIfMajorPerformanceCaveat:m};if("setAttribute"in t&&t.setAttribute("data-engine",`three.js r${"186"}`),t.addEventListener("webglcontextlost",vt,!1),t.addEventListener("webglcontextrestored",rt,!1),t.addEventListener("webglcontextcreationerror",Ft,!1),W===null){let G="webgl2";if(W=Qt(G,E),W===null)throw Qt(G)?new Error("THREE.WebGLRenderer: Error creating WebGL context with your selected attributes."):new Error("THREE.WebGLRenderer: Error creating WebGL context.")}we()}catch(E){throw t.removeEventListener("webglcontextlost",vt,!1),t.removeEventListener("webglcontextrestored",rt,!1),t.removeEventListener("webglcontextcreationerror",Ft,!1),Xe("WebGLRenderer: "+E.message),E}function we(){at=new sx(W),at.init(),xe=new Xv(W,at),N=new Zg(W,at,e,xe),b=new Gv(W,at),N.reversedDepthBuffer&&u&&b.buffers.depth.setReversed(!0),L=W.createFramebuffer(),U=W.createFramebuffer(),F=W.createFramebuffer(),Z=new lx(W),te=new Cv,ae=new Wv(W,at,b,te,N,xe,Z),ve=new rx(y),Me=new um(W),_e=new $g(W,Me),R=new ox(W,Me,Z,_e),I=new ux(W,R,Me,_e,Z),B=new cx(W,N,ae),re=new Kg(te),O=new Rv(y,ve,at,N,_e,re),z=new Zv(y,te),X=new Iv,Y=new Fv(at),ge=new qg(y,ve,b,I,_,c),fe=new Vv(y,I,N),he=new Kv(W,Z,N,b),ue=new Yg(W,at,Z),oe=new ax(W,at,Z),Z.programs=O.programs,y.capabilities=N,y.extensions=at,y.properties=te,y.renderLists=X,y.shadowMap=fe,y.state=b,y.info=Z}x!==Ln&&(M=new dx(x,t.width,t.height,a,r,s));let Re=new gu(y,W);this.xr=Re,this.getContext=function(){return W},this.getContextAttributes=function(){return W.getContextAttributes()},this.forceContextLoss=function(){let E=at.get("WEBGL_lose_context");E&&E.loseContext()},this.forceContextRestore=function(){let E=at.get("WEBGL_lose_context");E&&E.restoreContext()},this.getPixelRatio=function(){return le},this.setPixelRatio=function(E){E!==void 0&&(le=E,this.setSize(Je,se,!1))},this.getSize=function(E){return E.set(Je,se)},this.setSize=function(E,G,ie=!0){if(Re.isPresenting){Ge("WebGLRenderer: Can't change size while VR device is presenting.");return}Je=E,se=G,t.width=Math.floor(E*le),t.height=Math.floor(G*le),ie===!0&&(t.style.width=E+"px",t.style.height=G+"px"),M!==null&&M.setSize(t.width,t.height),this.setViewport(0,0,E,G)},this.getDrawingBufferSize=function(E){return E.set(Je*le,se*le).floor()},this.setDrawingBufferSize=function(E,G,ie){Je=E,se=G,le=ie,t.width=Math.floor(E*ie),t.height=Math.floor(G*ie),this.setViewport(0,0,E,G)},this.setEffects=function(E){if(x===Ln){Xe("WebGLRenderer: setEffects() requires outputBufferType set to HalfFloatType or FloatType.");return}if(E){for(let G=0;G<E.length;G++)if(E[G].isOutputPass===!0){Ge("WebGLRenderer: OutputPass is not needed in setEffects(). Tone mapping and color space conversion are applied automatically.");break}}M.setEffects(E||[])},this.getCurrentViewport=function(E){return E.copy(ce)},this.getViewport=function(E){return E.copy(ye)},this.setViewport=function(E,G,ie,j){E.isVector4?ye.set(E.x,E.y,E.z,E.w):ye.set(E,G,ie,j),b.viewport(ce.copy(ye).multiplyScalar(le).round())},this.getScissor=function(E){return E.copy(je)},this.setScissor=function(E,G,ie,j){E.isVector4?je.set(E.x,E.y,E.z,E.w):je.set(E,G,ie,j),b.scissor(pe.copy(je).multiplyScalar(le).round())},this.getScissorTest=function(){return Nt},this.setScissorTest=function(E){b.setScissorTest(Nt=E)},this.setOpaqueSort=function(E){Se=E},this.setTransparentSort=function(E){ke=E},this.getClearColor=function(E){return E.copy(ge.getClearColor())},this.setClearColor=function(){ge.setClearColor(...arguments)},this.getClearAlpha=function(){return ge.getClearAlpha()},this.setClearAlpha=function(){ge.setClearAlpha(...arguments)},this.clear=function(E=!0,G=!0,ie=!0){let j=0;if(E){let Q=!1;if(ee!==null){let Ee=ee.texture.format;Q=f.has(Ee)}if(Q){let Ee=ee.texture.type,Ie=d.has(Ee),Ae=ge.getClearColor(),De=ge.getClearAlpha(),Fe=Ae.r,et=Ae.g,st=Ae.b;Ie?(S[0]=Fe,S[1]=et,S[2]=st,S[3]=De,W.clearBufferuiv(W.COLOR,0,S)):(P[0]=Fe,P[1]=et,P[2]=st,P[3]=De,W.clearBufferiv(W.COLOR,0,P))}else j|=W.COLOR_BUFFER_BIT}G&&(j|=W.DEPTH_BUFFER_BIT,this.state.buffers.depth.setMask(!0)),ie&&(j|=W.STENCIL_BUFFER_BIT,this.state.buffers.stencil.setMask(4294967295)),j!==0&&W.clear(j)},this.clearColor=function(){this.clear(!0,!1,!1)},this.clearDepth=function(){this.clear(!1,!0,!1)},this.clearStencil=function(){this.clear(!1,!1,!0)},this.setNodesHandler=function(E){E.setRenderer(this),k=E},this.dispose=function(){t.removeEventListener("webglcontextlost",vt,!1),t.removeEventListener("webglcontextrestored",rt,!1),t.removeEventListener("webglcontextcreationerror",Ft,!1),ge.dispose(),X.dispose(),Y.dispose(),te.dispose(),ve.dispose(),I.dispose(),_e.dispose(),he.dispose(),O.dispose(),Re.dispose(),Re.removeEventListener("sessionstart",Be),Re.removeEventListener("sessionend",pt),$e.stop()};function vt(E){E.preventDefault(),Zc("WebGLRenderer: Context Lost."),C=!0}function rt(){Zc("WebGLRenderer: Context Restored."),C=!1;let E=Z.autoReset,G=fe.enabled,ie=fe.autoUpdate,j=fe.needsUpdate,Q=fe.type;we(),Z.autoReset=E,fe.enabled=G,fe.autoUpdate=ie,fe.needsUpdate=j,fe.type=Q}function Ft(E){Xe("WebGLRenderer: A WebGL context could not be created. Reason: ",E.statusMessage)}function wn(E){let G=E.target;G.removeEventListener("dispose",wn),$(G)}function $(E){de(E),te.remove(E)}function de(E){let G=te.get(E).programs;G!==void 0&&(G.forEach(function(ie){O.releaseProgram(ie)}),E.isShaderMaterial&&O.releaseShaderCache(E))}this.renderBufferDirect=function(E,G,ie,j,Q,Ee){G===null&&(G=$t);let Ie=Q.isMesh&&Q.matrixWorld.determinantAffine()<0,Ae=vs(E,G,ie,j,Q);b.setMaterial(j,Ie);let De=ie.index,Fe=1;if(j.wireframe===!0){if(De=R.getWireframeAttribute(ie),De===void 0)return;Fe=2}let et=ie.drawRange,st=ie.attributes.position,Ne=et.start*Fe,yt=(et.start+et.count)*Fe;Ee!==null&&(Ne=Math.max(Ne,Ee.start*Fe),yt=Math.min(yt,(Ee.start+Ee.count)*Fe)),De!==null?(Ne=Math.max(Ne,0),yt=Math.min(yt,De.count)):st!=null&&(Ne=Math.max(Ne,0),yt=Math.min(yt,st.count));let tn=yt-Ne;if(tn<0||tn===1/0)return;_e.setup(Q,j,Ae,ie,De);let Lt,Rt=ue;if(De!==null&&(Lt=Me.get(De),Rt=oe,Rt.setIndex(Lt)),Q.isMesh)j.wireframe===!0?(b.setLineWidth(j.wireframeLinewidth*Ut()),Rt.setMode(W.LINES)):Rt.setMode(W.TRIANGLES);else if(Q.isLine){let xn=j.linewidth;xn===void 0&&(xn=1),b.setLineWidth(xn*Ut()),Q.isLineSegments?Rt.setMode(W.LINES):Q.isLineLoop?Rt.setMode(W.LINE_LOOP):Rt.setMode(W.LINE_STRIP)}else Q.isPoints?Rt.setMode(W.POINTS):Q.isSprite&&Rt.setMode(W.TRIANGLES);if(Q.isBatchedMesh)if(at.get("WEBGL_multi_draw"))Rt.renderMultiDraw(Q._multiDrawStarts,Q._multiDrawCounts,Q._multiDrawCount);else{let xn=Q._multiDrawStarts,Ce=Q._multiDrawCounts,En=Q._multiDrawCount,mt=De?Me.get(De).bytesPerElement:1,Vn=te.get(j).currentProgram.getUniforms();for(let ai=0;ai<En;ai++)Vn.setValue(W,"_gl_DrawID",ai),Rt.render(xn[ai]/mt,Ce[ai])}else if(Q.isInstancedMesh)Rt.renderInstances(Ne,tn,Q.count);else if(ie.isInstancedBufferGeometry){let xn=ie._maxInstanceCount!==void 0?ie._maxInstanceCount:1/0,Ce=Math.min(ie.instanceCount,xn);Rt.renderInstances(Ne,tn,Ce)}else Rt.render(Ne,tn)};function be(E,G,ie,j){k!==null&&E.isNodeMaterial&&k.setObject(j,E),nt===!0&&re.setState(E,ie,!1),E.transparent===!0&&E.side===qn&&E.forceSinglePass===!1?(E.side=Rn,E.needsUpdate=!0,Er(E,G,j),E.side=Zi,E.needsUpdate=!0,Er(E,G,j),E.side=qn):Er(E,G,j)}this.compile=function(E,G,ie=null){ie===null&&(ie=E),k!==null&&k.renderStart(E,G,ie),A=Y.get(ie),A.init(G),p.push(A),ie.traverseVisible(function(Q){Q.isLight&&Q.layers.test(G.layers)&&(A.pushLight(Q),Q.castShadow&&A.pushShadow(Q))}),E!==ie&&E.traverseVisible(function(Q){Q.isLight&&Q.layers.test(G.layers)&&(A.pushLight(Q),Q.castShadow&&A.pushShadow(Q))}),A.setupLights(),k!==null&&k.updateLights(A.state.lightsArray),ft=this.localClippingEnabled,nt=re.init(this.clippingPlanes,ft),nt===!0&&re.setGlobalState(this.clippingPlanes,G),k!==null&&fe.render(A.state.shadowsArray,ie,G);let j=new Set;return E.traverse(function(Q){if(!(Q.isMesh||Q.isPoints||Q.isLine||Q.isSprite))return;let Ee=Q.material;if(Ee)if(Array.isArray(Ee))for(let Ie=0;Ie<Ee.length;Ie++){let Ae=Ee[Ie];be(Ae,ie,G,Q),j.add(Ae)}else be(Ee,ie,G,Q),j.add(Ee)}),A=p.pop(),k!==null&&k.renderEnd(),j},this.compileAsync=function(E,G,ie=null){let j=this.compile(E,G,ie);return new Promise(Q=>{function Ee(){if(j.forEach(function(Ie){let De=te.get(Ie).currentProgram;(De===void 0||De.isReady())&&j.delete(Ie)}),j.size===0){Q(E);return}setTimeout(Ee,10)}at.get("KHR_parallel_shader_compile")!==null?Ee():setTimeout(Ee,10)})};let K=null;function Pe(E){K&&K(E)}function Be(){$e.stop()}function pt(){$e.start()}let $e=new zd;$e.setAnimationLoop(Pe),typeof self<"u"&&$e.setContext(self),this.setAnimationLoop=function(E){K=E,Re.setAnimationLoop(E),E===null?$e.stop():$e.start()},Re.addEventListener("sessionstart",Be),Re.addEventListener("sessionend",pt),this.render=function(E,G){if(G!==void 0&&G.isCamera!==!0){Xe("WebGLRenderer.render: camera is not an instance of THREE.Camera.");return}if(C===!0)return;k!==null&&k.renderStart(E,G);let ie=Re.enabled===!0&&Re.isPresenting===!0,j=M!==null&&(ee===null||ie)&&M.begin(y,ee);if(E.matrixWorldAutoUpdate===!0&&E.updateMatrixWorld(),G.parent===null&&G.matrixWorldAutoUpdate===!0&&G.updateMatrixWorld(),Re.enabled===!0&&Re.isPresenting===!0&&(M===null||M.isCompositing()===!1)&&(Re.cameraAutoUpdate===!0&&Re.updateCamera(G),G=Re.getCamera()),E.isScene===!0&&E.onBeforeRender(y,E,G,ee),A=Y.get(E,p.length),A.init(G),A.state.textureUnits=ae.getTextureUnits(),p.push(A),Qe.multiplyMatrices(G.projectionMatrix,G.matrixWorldInverse),Ye.setFromProjectionMatrix(Qe,ei,G.reversedDepth),ft=this.localClippingEnabled,nt=re.init(this.clippingPlanes,ft),T=X.get(E,D.length),T.init(),D.push(T),Re.enabled===!0&&Re.isPresenting===!0){let Ie=y.xr.getDepthSensingMesh();Ie!==null&&en(Ie,G,-1/0,y.sortObjects)}en(E,G,0,y.sortObjects),T.finish(),k!==null&&k.updateLights(A.state.lightsArray),y.sortObjects===!0&&T.sort(Se,ke),wt=Re.enabled===!1||Re.isPresenting===!1||Re.hasDepthSensing()===!1,wt&&ge.addToRenderList(T,E),this.info.render.frame++,this.info.autoReset===!0&&this.info.reset(),nt===!0&&re.beginShadows();let Q=A.state.shadowsArray;if(fe.render(Q,E,G),nt===!0&&re.endShadows(),(j&&M.hasRenderPass())===!1){let Ie=T.opaque,Ae=T.transmissive;if(A.setupLights(),G.isArrayCamera){let De=G.cameras;if(Ae.length>0)for(let Fe=0,et=De.length;Fe<et;Fe++){let st=De[Fe];sn(Ie,Ae,E,st)}wt&&ge.render(E);for(let Fe=0,et=De.length;Fe<et;Fe++){let st=De[Fe];Ct(T,E,st,st.viewport)}}else Ae.length>0&&sn(Ie,Ae,E,G),wt&&ge.render(E),Ct(T,E,G)}ee!==null&&q===0&&(ae.updateMultisampleRenderTarget(ee),ae.updateRenderTargetMipmap(ee)),j&&M.end(y),E.isScene===!0&&E.onAfterRender(y,E,G),_e.resetDefaultState(),J=-1,ne=null,p.pop(),p.length>0?(A=p[p.length-1],ae.setTextureUnits(A.state.textureUnits),nt===!0&&re.setGlobalState(y.clippingPlanes,A.state.camera)):A=null,D.pop(),D.length>0?T=D[D.length-1]:T=null,k!==null&&k.renderEnd()};function en(E,G,ie,j){if(E.visible===!1)return;if(E.layers.test(G.layers)){if(E.isGroup)ie=E.renderOrder;else if(E.isLOD)E.autoUpdate===!0&&E.update(G);else if(E.isLightProbeGrid)A.pushLightProbeGrid(E);else if(E.isLight)A.pushLight(E),E.castShadow&&A.pushShadow(E);else if(E.isSprite){if(!E.frustumCulled||E.intersectsFrustum(Ye)){j&&Bt.setFromMatrixPosition(E.matrixWorld).applyMatrix4(Qe);let Ie=I.update(E),Ae=E.material;Ae.visible&&T.push(E,Ie,Ae,ie,Bt.z,null,G)}}else if((E.isMesh||E.isLine||E.isPoints)&&(!E.frustumCulled||E.intersectsFrustum(Ye))){let Ie=I.update(E),Ae=E.material;if(j&&(E.boundingSphere!==void 0?(E.boundingSphere===null&&E.computeBoundingSphere(),Bt.copy(E.boundingSphere.center)):(Ie.boundingSphere===null&&Ie.computeBoundingSphere(),Bt.copy(Ie.boundingSphere.center)),Bt.applyMatrix4(E.matrixWorld).applyMatrix4(Qe)),Array.isArray(Ae)){let De=Ie.groups;for(let Fe=0,et=De.length;Fe<et;Fe++){let st=De[Fe],Ne=Ae[st.materialIndex];Ne&&Ne.visible&&T.push(E,Ie,Ne,ie,Bt.z,st,G)}}else Ae.visible&&T.push(E,Ie,Ae,ie,Bt.z,null,G)}}let Ee=E.children;for(let Ie=0,Ae=Ee.length;Ie<Ae;Ie++)en(Ee[Ie],G,ie,j)}function Ct(E,G,ie,j){let{opaque:Q,transmissive:Ee,transparent:Ie}=E;A.setupLightsView(ie),nt===!0&&re.setGlobalState(y.clippingPlanes,ie),j&&b.viewport(ce.copy(j)),Q.length>0&&Hn(Q,G,ie),Ee.length>0&&Hn(Ee,G,ie),Ie.length>0&&Hn(Ie,G,ie),b.buffers.depth.setTest(!0),b.buffers.depth.setMask(!0),b.buffers.color.setMask(!0),b.setPolygonOffset(!1)}function sn(E,G,ie,j){if((ie.isScene===!0?ie.overrideMaterial:null)!==null)return;if(A.state.transmissionRenderTarget[j.id]===void 0){let Ne=at.has("EXT_color_buffer_half_float")||at.has("EXT_color_buffer_float");A.state.transmissionRenderTarget[j.id]=new In(1,1,{generateMipmaps:!0,type:Ne?si:Ln,minFilter:Ji,samples:Math.max(4,N.samples),stencilBuffer:s,resolveDepthBuffer:!1,resolveStencilBuffer:!1,storeMultisampledDepthBuffer:!1,storeMultisampledStencilBuffer:!1,colorSpace:lt.workingColorSpace})}let Ee=A.state.transmissionRenderTarget[j.id],Ie=j.viewport||ce;Ee.setSize(Ie.z*y.transmissionResolutionScale,Ie.w*y.transmissionResolutionScale);let Ae=y.getRenderTarget(),De=y.getActiveCubeFace(),Fe=y.getActiveMipmapLevel();y.setRenderTarget(Ee),y.getClearColor(qe),Le=y.getClearAlpha(),Le<1&&y.setClearColor(16777215,.5),y.clear(),wt&&ge.render(ie);let et=y.toneMapping;y.toneMapping=ni;let st=j.viewport;if(j.viewport!==void 0&&(j.viewport=void 0),A.setupLightsView(j),nt===!0&&re.setGlobalState(y.clippingPlanes,j),Hn(E,ie,j),ae.updateMultisampleRenderTarget(Ee),ae.updateRenderTargetMipmap(Ee),at.has("WEBGL_multisampled_render_to_texture")===!1){let Ne=!1;for(let yt=0,tn=G.length;yt<tn;yt++){let Lt=G[yt],{object:Rt,geometry:xn,material:Ce,group:En}=Lt;if(Ce.side===qn&&Rt.layers.test(j.layers)){let mt=Ce.side;Ce.side=Rn,Ce.needsUpdate=!0,Li(Rt,ie,j,xn,Ce,En),Ce.side=mt,Ce.needsUpdate=!0,Ne=!0}}Ne===!0&&(ae.updateMultisampleRenderTarget(Ee),ae.updateRenderTargetMipmap(Ee))}y.setRenderTarget(Ae,De,Fe),y.setClearColor(qe,Le),st!==void 0&&(j.viewport=st),y.toneMapping=et}function Hn(E,G,ie){let j=G.isScene===!0?G.overrideMaterial:null;for(let Q=0,Ee=E.length;Q<Ee;Q++){let Ie=E[Q],{object:Ae,geometry:De,group:Fe}=Ie,et=Ie.material;et.allowOverride===!0&&j!==null&&(et=j),Ae.layers.test(ie.layers)&&Li(Ae,G,ie,De,et,Fe)}}function Li(E,G,ie,j,Q,Ee){k!==null&&Q.isNodeMaterial&&k.setObject(E,Q),E.onBeforeRender(y,G,ie,j,Q,Ee),E.modelViewMatrix.multiplyMatrices(ie.matrixWorldInverse,E.matrixWorld),E.normalMatrix.getNormalMatrix(E.modelViewMatrix),Q.onBeforeRender(y,G,ie,j,E,Ee),Q.transparent===!0&&Q.side===qn&&Q.forceSinglePass===!1?(Q.side=Rn,Q.needsUpdate=!0,y.renderBufferDirect(ie,G,j,Q,E,Ee),Q.side=Zi,Q.needsUpdate=!0,y.renderBufferDirect(ie,G,j,Q,E,Ee),Q.side=qn):y.renderBufferDirect(ie,G,j,Q,E,Ee),E.onAfterRender(y,G,ie,j,Q,Ee)}function Er(E,G,ie){G.isScene!==!0&&(G=$t);let j=te.get(E),Q=A.state.lights,Ee=A.state.shadowsArray,Ie=Q.state.version,Ae=O.getParameters(E,Q.state,Ee,G,ie,A.state.lightProbeGridArray),De=O.getProgramCacheKey(Ae),Fe=j.programs;j.environment=E.isMeshStandardMaterial||E.isMeshLambertMaterial||E.isMeshPhongMaterial?G.environment:null,j.fog=G.fog;let et=E.isMeshStandardMaterial||E.isMeshLambertMaterial&&!E.envMap||E.isMeshPhongMaterial&&!E.envMap;j.envMap=ve.get(E.envMap||j.environment,et),j.envMapRotation=j.environment!==null&&E.envMap===null?G.environmentRotation:E.envMapRotation,Fe===void 0&&(E.addEventListener("dispose",wn),Fe=new Map,j.programs=Fe);let st=Fe.get(De);if(st!==void 0){if(j.currentProgram===st&&j.lightsStateVersion===Ie)return Tr(E,Ae),st}else Ae.uniforms=O.getUniforms(E),k!==null&&E.isNodeMaterial&&k.build(E,ie,Ae),E.onBeforeCompile(Ae,y),st=O.acquireProgram(Ae,De),Fe.set(De,st),j.uniforms=Ae.uniforms;let Ne=j.uniforms;return(!E.isShaderMaterial&&!E.isRawShaderMaterial||E.clipping===!0)&&(Ne.clippingPlanes=re.uniform),Tr(E,Ae),j.needsLights=Ar(E),j.lightsStateVersion=Ie,j.needsLights&&(Ne.ambientLightColor.value=Q.state.ambient,Ne.lightProbe.value=Q.state.probe,Ne.sunLights.value=Q.state.sun,Ne.sunLightShadows.value=Q.state.sunShadow,Ne.directionalLights.value=Q.state.directional,Ne.directionalLightShadows.value=Q.state.directionalShadow,Ne.spotLights.value=Q.state.spot,Ne.spotLightShadows.value=Q.state.spotShadow,Ne.rectAreaLights.value=Q.state.rectArea,Ne.ltc_1.value=Q.state.rectAreaLTC1,Ne.ltc_2.value=Q.state.rectAreaLTC2,Ne.pointLights.value=Q.state.point,Ne.pointLightShadows.value=Q.state.pointShadow,Ne.hemisphereLights.value=Q.state.hemi,Ne.sunShadowMatrix.value=Q.state.sunShadowMatrix,Ne.sunShadowCascade.value=Q.state.sunShadowCascade,Ne.directionalShadowMatrix.value=Q.state.directionalShadowMatrix,Ne.spotLightMatrix.value=Q.state.spotLightMatrix,Ne.spotLightMap.value=Q.state.spotLightMap,Ne.pointShadowMatrix.value=Q.state.pointShadowMatrix),j.lightProbeGrid=A.state.lightProbeGridArray.length>0,j.currentProgram=st,j.uniformsList=null,st}function xs(E){if(E.uniformsList===null){let G=E.currentProgram.getUniforms();E.uniformsList=ps.seqWithValue(G.seq,E.uniforms)}return E.uniformsList}function Tr(E,G){let ie=te.get(E);ie.outputColorSpace=G.outputColorSpace,ie.batching=G.batching,ie.batchingColor=G.batchingColor,ie.instancing=G.instancing,ie.instancingColor=G.instancingColor,ie.instancingMorph=G.instancingMorph,ie.skinning=G.skinning,ie.morphTargets=G.morphTargets,ie.morphNormals=G.morphNormals,ie.morphColors=G.morphColors,ie.morphTargetsCount=G.morphTargetsCount,ie.numClippingPlanes=G.numClippingPlanes,ie.numIntersection=G.numClipIntersection,ie.vertexAlphas=G.vertexAlphas,ie.vertexTangents=G.vertexTangents,ie.toneMapping=G.toneMapping}function tr(E,G){if(E.length===0)return null;if(E.length===1)return E[0].texture!==null?E[0]:null;w.setFromMatrixPosition(G.matrixWorld);for(let ie=0,j=E.length;ie<j;ie++){let Q=E[ie];if(Q.texture!==null&&Q.boundingBox.containsPoint(w))return Q}return null}function vs(E,G,ie,j,Q){G.isScene!==!0&&(G=$t),ae.resetTextureUnits();let Ee=G.fog,Ie=j.isMeshStandardMaterial||j.isMeshLambertMaterial||j.isMeshPhongMaterial?G.environment:null,Ae=ee===null?y.outputColorSpace:ee.isXRRenderTarget===!0?ee.texture.colorSpace:lt.workingColorSpace,De=j.isMeshStandardMaterial||j.isMeshLambertMaterial&&!j.envMap||j.isMeshPhongMaterial&&!j.envMap,Fe=ve.get(j.envMap||Ie,De),et=j.vertexColors===!0&&!!ie.attributes.color&&ie.attributes.color.itemSize===4,st=!!ie.attributes.tangent&&(!!j.normalMap||j.anisotropy>0),Ne=!!ie.morphAttributes.position,yt=!!ie.morphAttributes.normal,tn=!!ie.morphAttributes.color,Lt=ni;j.toneMapped&&(ee===null||ee.isXRRenderTarget===!0)&&(Lt=y.toneMapping);let Rt=ie.morphAttributes.position||ie.morphAttributes.normal||ie.morphAttributes.color,xn=Rt!==void 0?Rt.length:0,Ce=te.get(j),En=A.state.lights;if(nt===!0&&(ft===!0||E!==ne)){let Pt=E===ne&&j.id===J;re.setState(j,E,Pt)}let mt=!1;j.version===Ce.__version?(Ce.needsLights&&Ce.lightsStateVersion!==En.state.version||Ce.outputColorSpace!==Ae||Q.isBatchedMesh&&Ce.batching===!1||!Q.isBatchedMesh&&Ce.batching===!0||Q.isBatchedMesh&&Ce.batchingColor===!0&&Q._colorsTexture===null||Q.isBatchedMesh&&Ce.batchingColor===!1&&Q._colorsTexture!==null||Q.isInstancedMesh&&Ce.instancing===!1||!Q.isInstancedMesh&&Ce.instancing===!0||Q.isSkinnedMesh&&Ce.skinning===!1||!Q.isSkinnedMesh&&Ce.skinning===!0||Q.isInstancedMesh&&Ce.instancingColor===!0&&Q.instanceColor===null||Q.isInstancedMesh&&Ce.instancingColor===!1&&Q.instanceColor!==null||Q.isInstancedMesh&&Ce.instancingMorph===!0&&Q.morphTexture===null||Q.isInstancedMesh&&Ce.instancingMorph===!1&&Q.morphTexture!==null||Ce.envMap!==Fe||j.fog===!0&&Ce.fog!==Ee||Ce.numClippingPlanes!==void 0&&(Ce.numClippingPlanes!==re.numPlanes||Ce.numIntersection!==re.numIntersection)||Ce.vertexAlphas!==et||Ce.vertexTangents!==st||Ce.morphTargets!==Ne||Ce.morphNormals!==yt||Ce.morphColors!==tn||Ce.toneMapping!==Lt||Ce.morphTargetsCount!==xn||!!Ce.lightProbeGrid!=A.state.lightProbeGridArray.length>0)&&(mt=!0):(mt=!0,Ce.__version=j.version);let Vn=Ce.currentProgram;mt===!0&&(Vn=Er(j,G,Q),k&&j.isNodeMaterial&&k.onUpdateProgram(j,Vn,Ce));let ai=!1,ki=!1,Rr=!1,At=Vn.getUniforms(),Yt=Ce.uniforms;if(b.useProgram(Vn.program)&&(ai=!0,ki=!0,Rr=!0),j.id!==J&&(J=j.id,ki=!0),Ce.needsLights){let Pt=tr(A.state.lightProbeGridArray,Q);Ce.lightProbeGrid!==Pt&&(Ce.lightProbeGrid=Pt,ki=!0)}if(ai||ne!==E){b.buffers.depth.getReversed()&&E.reversedDepth!==!0&&(E._reversedDepth=!0,E.updateProjectionMatrix()),At.setValue(W,"projectionMatrix",E.projectionMatrix),At.setValue(W,"viewMatrix",E.matrixWorldInverse);let Ni=At.map.cameraPosition;Ni!==void 0&&Ni.setValue(W,Tt.setFromMatrixPosition(E.matrixWorld)),N.logarithmicDepthBuffer&&At.setValue(W,"logDepthBufFC",2/(Math.log(E.far+1)/Math.LN2)),(j.isMeshPhongMaterial||j.isMeshToonMaterial||j.isMeshLambertMaterial||j.isMeshBasicMaterial||j.isMeshStandardMaterial||j.isShaderMaterial)&&At.setValue(W,"isOrthographic",E.isOrthographicCamera===!0),ne!==E&&(ne=E,ki=!0,Rr=!0)}if(Ce.needsLights&&(En.state.sunShadowMap.length>0&&At.setValue(W,"sunShadowMap",En.state.sunShadowMap,ae),En.state.directionalShadowMap.length>0&&At.setValue(W,"directionalShadowMap",En.state.directionalShadowMap,ae),En.state.spotShadowMap.length>0&&At.setValue(W,"spotShadowMap",En.state.spotShadowMap,ae),En.state.pointShadowMap.length>0&&At.setValue(W,"pointShadowMap",En.state.pointShadowMap,ae)),Q.isSkinnedMesh){At.setOptional(W,Q,"bindMatrix"),At.setOptional(W,Q,"bindMatrixInverse");let Pt=Q.skeleton;Pt&&(Pt.boneTexture===null&&Pt.computeBoneTexture(),At.setValue(W,"boneTexture",Pt.boneTexture,ae))}Q.isBatchedMesh&&(At.setOptional(W,Q,"batchingTexture"),At.setValue(W,"batchingTexture",Q._matricesTexture,ae),At.setOptional(W,Q,"batchingIdTexture"),At.setValue(W,"batchingIdTexture",Q._indirectTexture,ae),At.setOptional(W,Q,"batchingColorTexture"),Q._colorsTexture!==null&&At.setValue(W,"batchingColorTexture",Q._colorsTexture,ae));let Di=ie.morphAttributes;if((Di.position!==void 0||Di.normal!==void 0||Di.color!==void 0)&&B.update(Q,ie,Vn),(ki||Ce.receiveShadow!==Q.receiveShadow)&&(Ce.receiveShadow=Q.receiveShadow,At.setValue(W,"receiveShadow",Q.receiveShadow)),(j.isMeshStandardMaterial||j.isMeshLambertMaterial||j.isMeshPhongMaterial)&&j.envMap===null&&G.environment!==null&&(Yt.envMapIntensity.value=G.environmentIntensity),Yt.dfgLUT!==void 0&&(Yt.dfgLUT.value=jv()),ki){if(At.setValue(W,"toneMappingExposure",y.toneMappingExposure),Ce.needsLights&&_s(Yt,Rr),Ee&&j.fog===!0&&z.refreshFogUniforms(Yt,Ee),z.refreshMaterialUniforms(Yt,j,le,se,A.state.transmissionRenderTarget[E.id]),Ce.needsLights&&Ce.lightProbeGrid){let Pt=Ce.lightProbeGrid;Yt.probesSH.value=Pt.texture,Yt.probesMin.value.copy(Pt.boundingBox.min),Yt.probesMax.value.copy(Pt.boundingBox.max),Yt.probesResolution.value.copy(Pt.resolution)}ps.upload(W,xs(Ce),Yt,ae)}if(j.isShaderMaterial&&j.uniformsNeedUpdate===!0&&(ps.upload(W,xs(Ce),Yt,ae),j.uniformsNeedUpdate=!1),j.isSpriteMaterial&&At.setValue(W,"center",Q.center),At.setValue(W,"modelViewMatrix",Q.modelViewMatrix),At.setValue(W,"normalMatrix",Q.normalMatrix),At.setValue(W,"modelMatrix",Q.matrixWorld),j.uniformsGroups!==void 0){let Pt=j.uniformsGroups;for(let Ni=0,Cr=Pt.length;Ni<Cr;Ni++){let Du=Pt[Ni];he.update(Du,Vn),he.bind(Du,Vn)}}return Vn}function _s(E,G){E.ambientLightColor.needsUpdate=G,E.lightProbe.needsUpdate=G,E.sunLights.needsUpdate=G,E.sunLightShadows.needsUpdate=G,E.directionalLights.needsUpdate=G,E.directionalLightShadows.needsUpdate=G,E.pointLights.needsUpdate=G,E.pointLightShadows.needsUpdate=G,E.spotLights.needsUpdate=G,E.spotLightShadows.needsUpdate=G,E.rectAreaLights.needsUpdate=G,E.hemisphereLights.needsUpdate=G}function Ar(E){return E.isMeshLambertMaterial||E.isMeshToonMaterial||E.isMeshPhongMaterial||E.isMeshStandardMaterial||E.isShadowMaterial||E.isShaderMaterial&&E.lights===!0}this.getActiveCubeFace=function(){return H},this.getActiveMipmapLevel=function(){return q},this.getRenderTarget=function(){return ee},this.setRenderTargetTextures=function(E,G,ie){let j=te.get(E);j.__autoAllocateDepthBuffer=E.resolveDepthBuffer===!1,j.__autoAllocateDepthBuffer===!1&&(j.__useRenderToTexture=!1),te.get(E.texture).__webglTexture=G,te.get(E.depthTexture).__webglTexture=j.__autoAllocateDepthBuffer?void 0:ie,j.__hasExternalTextures=!0},this.setRenderTargetFramebuffer=function(E,G){let ie=te.get(E);ie.__webglFramebuffer=G,ie.__useDefaultFramebuffer=G===void 0},this.setRenderTarget=function(E,G=0,ie=0){ee=E,H=G,q=ie;let j=null,Q=!1,Ee=!1;if(E){let Ae=te.get(E);if(Ae.__useDefaultFramebuffer!==void 0){b.bindFramebuffer(W.FRAMEBUFFER,Ae.__webglFramebuffer),ce.copy(E.viewport),pe.copy(E.scissor),me=E.scissorTest,b.viewport(ce),b.scissor(pe),b.setScissorTest(me),J=-1;return}else if(Ae.__webglFramebuffer===void 0)ae.setupRenderTarget(E);else if(Ae.__hasExternalTextures)ae.rebindTextures(E,te.get(E.texture).__webglTexture,te.get(E.depthTexture).__webglTexture);else if(E.depthBuffer){let et=E.depthTexture;if(Ae.__boundDepthTexture!==et){if(et!==null&&te.has(et)&&(E.width!==et.image.width||E.height!==et.image.height))throw new Error("THREE.WebGLRenderer: Attached DepthTexture is initialized to the incorrect size.");ae.setupDepthRenderbuffer(E)}}let De=E.texture;(De.isData3DTexture||De.isDataArrayTexture||De.isCompressedArrayTexture)&&(Ee=!0);let Fe=te.get(E).__webglFramebuffer;E.isWebGLCubeRenderTarget?(Array.isArray(Fe[G])?j=Fe[G][ie]:j=Fe[G],Q=!0):E.samples>0&&ae.useMultisampledRTT(E)===!1?j=te.get(E).__webglMultisampledFramebuffer:Array.isArray(Fe)?j=Fe[ie]:j=Fe,ce.copy(E.viewport),pe.copy(E.scissor),me=E.scissorTest}else ce.copy(ye).multiplyScalar(le).floor(),pe.copy(je).multiplyScalar(le).floor(),me=Nt;if(ie!==0&&(j=L),b.bindFramebuffer(W.FRAMEBUFFER,j)&&b.drawBuffers(E,j),b.viewport(ce),b.scissor(pe),b.setScissorTest(me),Q){let Ae=te.get(E.texture);W.framebufferTexture2D(W.FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_CUBE_MAP_POSITIVE_X+G,Ae.__webglTexture,ie)}else if(Ee){let Ae=G;for(let De=0;De<E.textures.length;De++){let Fe=te.get(E.textures[De]);W.framebufferTextureLayer(W.FRAMEBUFFER,W.COLOR_ATTACHMENT0+De,Fe.__webglTexture,ie,Ae)}}else if(E!==null&&ie!==0){let Ae=te.get(E.texture);W.framebufferTexture2D(W.FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_2D,Ae.__webglTexture,ie)}J=-1};function ys(E){let G=te.get(E);return(G.__readFormat!==E.format||G.__readType!==E.type)&&(G.__readFormat=E.format,G.__readType=E.type,G.__formatReadable=N.textureFormatReadable(E.format),G.__typeReadable=N.textureTypeReadable(E.type)),G}this.readRenderTargetPixels=function(E,G,ie,j,Q,Ee,Ie,Ae=0){if(!(E&&E.isWebGLRenderTarget)){Xe("WebGLRenderer.readRenderTargetPixels: renderTarget is not THREE.WebGLRenderTarget.");return}let De=te.get(E).__webglFramebuffer;if(E.isWebGLCubeRenderTarget&&Ie!==void 0&&(De=De[Ie]),De){b.bindFramebuffer(W.FRAMEBUFFER,De);try{let Fe=E.textures[Ae],et=Fe.format,st=Fe.type;E.textures.length>1&&W.readBuffer(W.COLOR_ATTACHMENT0+Ae);let Ne=ys(Fe);if(Ne.__formatReadable===!1){Xe("WebGLRenderer.readRenderTargetPixels: renderTarget is not in RGBA or implementation defined format.");return}if(Ne.__typeReadable===!1){Xe("WebGLRenderer.readRenderTargetPixels: renderTarget is not in UnsignedByteType or implementation defined type.");return}G>=0&&G<=E.width-j&&ie>=0&&ie<=E.height-Q&&W.readPixels(G,ie,j,Q,xe.convert(et),xe.convert(st),Ee)}finally{let Fe=ee!==null?te.get(ee).__webglFramebuffer:null;b.bindFramebuffer(W.FRAMEBUFFER,Fe)}}},this.readRenderTargetPixelsAsync=async function(E,G,ie,j,Q,Ee,Ie,Ae=0){if(!(E&&E.isWebGLRenderTarget))throw new Error("THREE.WebGLRenderer.readRenderTargetPixels: renderTarget is not THREE.WebGLRenderTarget.");let De=te.get(E).__webglFramebuffer;if(E.isWebGLCubeRenderTarget&&Ie!==void 0&&(De=De[Ie]),De)if(G>=0&&G<=E.width-j&&ie>=0&&ie<=E.height-Q){b.bindFramebuffer(W.FRAMEBUFFER,De);let Fe=E.textures[Ae],et=Fe.format,st=Fe.type;E.textures.length>1&&W.readBuffer(W.COLOR_ATTACHMENT0+Ae);let Ne=ys(Fe);if(Ne.__formatReadable===!1)throw new Error("THREE.WebGLRenderer.readRenderTargetPixelsAsync: renderTarget is not in RGBA or implementation defined format.");if(Ne.__typeReadable===!1)throw new Error("THREE.WebGLRenderer.readRenderTargetPixelsAsync: renderTarget is not in UnsignedByteType or implementation defined type.");let yt=W.createBuffer();W.bindBuffer(W.PIXEL_PACK_BUFFER,yt),W.bufferData(W.PIXEL_PACK_BUFFER,Ee.byteLength,W.STREAM_READ),W.readPixels(G,ie,j,Q,xe.convert(et),xe.convert(st),0),W.bindBuffer(W.PIXEL_PACK_BUFFER,null);let tn=ee!==null?te.get(ee).__webglFramebuffer:null;b.bindFramebuffer(W.FRAMEBUFFER,tn);let Lt=W.fenceSync(W.SYNC_GPU_COMMANDS_COMPLETE,0);return W.flush(),await fd(W,Lt,4),W.bindBuffer(W.PIXEL_PACK_BUFFER,yt),W.getBufferSubData(W.PIXEL_PACK_BUFFER,0,Ee),W.bindBuffer(W.PIXEL_PACK_BUFFER,null),W.deleteBuffer(yt),W.deleteSync(Lt),Ee}else throw new Error("THREE.WebGLRenderer.readRenderTargetPixelsAsync: requested read bounds are out of range.")},this.copyFramebufferToTexture=function(E,G=null,ie=0){let j=Math.pow(2,-ie),Q=Math.floor(E.image.width*j),Ee=Math.floor(E.image.height*j),Ie=G!==null?G.x:0,Ae=G!==null?G.y:0;ae.setTexture2D(E,0),W.copyTexSubImage2D(W.TEXTURE_2D,ie,0,0,Ie,Ae,Q,Ee),b.unbindTexture()},this.copyTextureToTexture=function(E,G,ie=null,j=null,Q=0,Ee=0){let Ie,Ae,De,Fe,et,st,Ne,yt,tn,Lt=E.isCompressedTexture?E.mipmaps[Ee]:E.image;if(ie!==null)Ie=ie.max.x-ie.min.x,Ae=ie.max.y-ie.min.y,De=ie.isBox3?ie.max.z-ie.min.z:1,Fe=ie.min.x,et=ie.min.y,st=ie.isBox3?ie.min.z:0;else{let Yt=Math.pow(2,-Q);Ie=Math.floor(Lt.width*Yt),Ae=Math.floor(Lt.height*Yt),E.isDataArrayTexture?De=Lt.depth:E.isData3DTexture?De=Math.floor(Lt.depth*Yt):De=1,Fe=0,et=0,st=0}j!==null?(Ne=j.x,yt=j.y,tn=j.z):(Ne=0,yt=0,tn=0);let Rt=xe.convert(G.format),xn=xe.convert(G.type),Ce;G.isData3DTexture?(ae.setTexture3D(G,0),Ce=W.TEXTURE_3D):G.isDataArrayTexture||G.isCompressedArrayTexture?(ae.setTexture2DArray(G,0),Ce=W.TEXTURE_2D_ARRAY):(ae.setTexture2D(G,0),Ce=W.TEXTURE_2D),b.activeTexture(W.TEXTURE0),b.pixelStorei(W.UNPACK_FLIP_Y_WEBGL,G.flipY),b.pixelStorei(W.UNPACK_PREMULTIPLY_ALPHA_WEBGL,G.premultiplyAlpha),b.pixelStorei(W.UNPACK_ALIGNMENT,G.unpackAlignment);let En=b.getParameter(W.UNPACK_ROW_LENGTH),mt=b.getParameter(W.UNPACK_IMAGE_HEIGHT),Vn=b.getParameter(W.UNPACK_SKIP_PIXELS),ai=b.getParameter(W.UNPACK_SKIP_ROWS),ki=b.getParameter(W.UNPACK_SKIP_IMAGES);b.pixelStorei(W.UNPACK_ROW_LENGTH,Lt.width),b.pixelStorei(W.UNPACK_IMAGE_HEIGHT,Lt.height),b.pixelStorei(W.UNPACK_SKIP_PIXELS,Fe),b.pixelStorei(W.UNPACK_SKIP_ROWS,et),b.pixelStorei(W.UNPACK_SKIP_IMAGES,st);let Rr=E.isDataArrayTexture||E.isData3DTexture,At=G.isDataArrayTexture||G.isData3DTexture;if(E.isDepthTexture){let Yt=te.get(E),Di=te.get(G),Pt=te.get(Yt.__renderTarget),Ni=te.get(Di.__renderTarget);b.bindFramebuffer(W.READ_FRAMEBUFFER,Pt.__webglFramebuffer),b.bindFramebuffer(W.DRAW_FRAMEBUFFER,Ni.__webglFramebuffer);for(let Cr=0;Cr<De;Cr++)Rr&&(W.framebufferTextureLayer(W.READ_FRAMEBUFFER,W.COLOR_ATTACHMENT0,te.get(E).__webglTexture,Q,st+Cr),W.framebufferTextureLayer(W.DRAW_FRAMEBUFFER,W.COLOR_ATTACHMENT0,te.get(G).__webglTexture,Ee,tn+Cr)),W.blitFramebuffer(Fe,et,Ie,Ae,Ne,yt,Ie,Ae,W.DEPTH_BUFFER_BIT,W.NEAREST);b.bindFramebuffer(W.READ_FRAMEBUFFER,null),b.bindFramebuffer(W.DRAW_FRAMEBUFFER,null)}else if(Q!==0||E.isRenderTargetTexture||te.has(E)){let Yt=te.get(E),Di=te.get(G);b.bindFramebuffer(W.READ_FRAMEBUFFER,U),b.bindFramebuffer(W.DRAW_FRAMEBUFFER,F);for(let Pt=0;Pt<De;Pt++)Rr?W.framebufferTextureLayer(W.READ_FRAMEBUFFER,W.COLOR_ATTACHMENT0,Yt.__webglTexture,Q,st+Pt):W.framebufferTexture2D(W.READ_FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_2D,Yt.__webglTexture,Q),At?W.framebufferTextureLayer(W.DRAW_FRAMEBUFFER,W.COLOR_ATTACHMENT0,Di.__webglTexture,Ee,tn+Pt):W.framebufferTexture2D(W.DRAW_FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_2D,Di.__webglTexture,Ee),Q!==0?W.blitFramebuffer(Fe,et,Ie,Ae,Ne,yt,Ie,Ae,W.COLOR_BUFFER_BIT,W.NEAREST):At?W.copyTexSubImage3D(Ce,Ee,Ne,yt,tn+Pt,Fe,et,Ie,Ae):W.copyTexSubImage2D(Ce,Ee,Ne,yt,Fe,et,Ie,Ae);b.bindFramebuffer(W.READ_FRAMEBUFFER,null),b.bindFramebuffer(W.DRAW_FRAMEBUFFER,null)}else At?E.isDataTexture||E.isData3DTexture?W.texSubImage3D(Ce,Ee,Ne,yt,tn,Ie,Ae,De,Rt,xn,Lt.data):G.isCompressedArrayTexture?W.compressedTexSubImage3D(Ce,Ee,Ne,yt,tn,Ie,Ae,De,Rt,Lt.data):W.texSubImage3D(Ce,Ee,Ne,yt,tn,Ie,Ae,De,Rt,xn,Lt):E.isDataTexture?W.texSubImage2D(W.TEXTURE_2D,Ee,Ne,yt,Ie,Ae,Rt,xn,Lt.data):E.isCompressedTexture?W.compressedTexSubImage2D(W.TEXTURE_2D,Ee,Ne,yt,Lt.width,Lt.height,Rt,Lt.data):W.texSubImage2D(W.TEXTURE_2D,Ee,Ne,yt,Ie,Ae,Rt,xn,Lt);b.pixelStorei(W.UNPACK_ROW_LENGTH,En),b.pixelStorei(W.UNPACK_IMAGE_HEIGHT,mt),b.pixelStorei(W.UNPACK_SKIP_PIXELS,Vn),b.pixelStorei(W.UNPACK_SKIP_ROWS,ai),b.pixelStorei(W.UNPACK_SKIP_IMAGES,ki),Ee===0&&G.generateMipmaps&&W.generateMipmap(Ce),b.unbindTexture()},this.initRenderTarget=function(E){te.get(E).__webglFramebuffer===void 0&&ae.setupRenderTarget(E)},this.initTexture=function(E){E.isCubeTexture?ae.setTextureCube(E,0):E.isData3DTexture?ae.setTexture3D(E,0):E.isDataArrayTexture||E.isCompressedArrayTexture?ae.setTexture2DArray(E,0):ae.setTexture2D(E,0),b.unbindTexture()},this.resetState=function(){H=0,q=0,ee=null,b.reset(),_e.reset()},typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("observe",{detail:this}))}get coordinateSystem(){return ei}get outputColorSpace(){return this._outputColorSpace}set outputColorSpace(e){this._outputColorSpace=e;let t=this.getContext();t.drawingBufferColorSpace=lt._getDrawingBufferColorSpace(e),t.unpackColorSpace=lt._getUnpackColorSpace()}};var $d={type:"change"},vu={type:"start"},Zd={type:"end"},Al=new Ri,Yd=new Un,Qv=Math.cos(70*Jc.DEG2RAD),cn=new V,kn=2*Math.PI,Et={NONE:-1,ROTATE:0,DOLLY:1,PAN:2,TOUCH_ROTATE:3,TOUCH_PAN:4,TOUCH_DOLLY_PAN:5,TOUCH_DOLLY_ROTATE:6},xu=1e-6,Rl=class extends Qs{constructor(e,t=null){super(e,t),this.state=Et.NONE,this.target=new V,this.cursor=new V,this.minDistance=0,this.maxDistance=1/0,this.minZoom=0,this.maxZoom=1/0,this.minTargetRadius=0,this.maxTargetRadius=1/0,this.minPolarAngle=0,this.maxPolarAngle=Math.PI,this.minAzimuthAngle=-1/0,this.maxAzimuthAngle=1/0,this.enableDamping=!1,this.dampingFactor=.05,this.enableZoom=!0,this.zoomSpeed=1,this.enableRotate=!0,this.rotateSpeed=1,this.keyRotateSpeed=1,this.enablePan=!0,this.panSpeed=1,this.screenSpacePanning=!0,this.keyPanSpeed=7,this.zoomToCursor=!1,this.autoRotate=!1,this.autoRotateSpeed=2,this.keys={LEFT:"ArrowLeft",UP:"ArrowUp",RIGHT:"ArrowRight",BOTTOM:"ArrowDown"},this.mouseButtons={LEFT:$i.ROTATE,MIDDLE:$i.DOLLY,RIGHT:$i.PAN},this.touches={ONE:Yi.ROTATE,TWO:Yi.DOLLY_PAN},this.target0=this.target.clone(),this.position0=this.object.position.clone(),this.zoom0=this.object.zoom,this._cursorStyle="auto",this._domElementKeyEvents=null,this._lastPosition=new V,this._lastQuaternion=new Fn,this._lastTargetPosition=new V,this._quat=new Fn().setFromUnitVectors(e.up,new V(0,1,0)),this._quatInverse=this._quat.clone().invert(),this._spherical=new ss,this._sphericalDelta=new ss,this._scale=1,this._panOffset=new V,this._rotateStart=new He,this._rotateEnd=new He,this._rotateDelta=new He,this._panStart=new He,this._panEnd=new He,this._panDelta=new He,this._dollyStart=new He,this._dollyEnd=new He,this._dollyDelta=new He,this._dollyDirection=new V,this._mouse=new He,this._performCursorZoom=!1,this._pointers=[],this._pointerPositions={},this._controlActive=!1,this._onPointerMove=t_.bind(this),this._onPointerDown=e_.bind(this),this._onPointerUp=n_.bind(this),this._onContextMenu=c_.bind(this),this._onMouseWheel=s_.bind(this),this._onKeyDown=o_.bind(this),this._onTouchStart=a_.bind(this),this._onTouchMove=l_.bind(this),this._onMouseDown=i_.bind(this),this._onMouseMove=r_.bind(this),this._interceptControlDown=u_.bind(this),this._interceptControlUp=h_.bind(this),this.domElement!==null&&this.connect(this.domElement),this.update()}set cursorStyle(e){this._cursorStyle=e,e==="grab"?this.domElement.style.cursor="grab":this.domElement.style.cursor="auto"}get cursorStyle(){return this._cursorStyle}connect(e){super.connect(e),this.domElement.addEventListener("pointerdown",this._onPointerDown),this.domElement.addEventListener("pointercancel",this._onPointerUp),this.domElement.addEventListener("contextmenu",this._onContextMenu),this.domElement.addEventListener("wheel",this._onMouseWheel,{passive:!1}),this.domElement.getRootNode().addEventListener("keydown",this._interceptControlDown,{passive:!0,capture:!0}),this.domElement.style.touchAction="none"}disconnect(){this.state=Et.NONE,this.domElement.removeEventListener("pointerdown",this._onPointerDown),this.domElement.ownerDocument.removeEventListener("pointermove",this._onPointerMove),this.domElement.ownerDocument.removeEventListener("pointerup",this._onPointerUp),this.domElement.removeEventListener("pointercancel",this._onPointerUp),this.domElement.removeEventListener("wheel",this._onMouseWheel),this.domElement.removeEventListener("contextmenu",this._onContextMenu),this.stopListenToKeyEvents();let e=this.domElement.getRootNode();e.removeEventListener("keydown",this._interceptControlDown,{capture:!0}),e.removeEventListener("keyup",this._interceptControlUp,{capture:!0}),this._controlActive=!1,this._pointers.length=0,this._pointerPositions={},this.domElement.style.touchAction="",this.domElement.style.cursor="auto"}dispose(){this.disconnect()}getPolarAngle(){return this._spherical.phi}getAzimuthalAngle(){return this._spherical.theta}getDistance(){return this.object.position.distanceTo(this.target)}listenToKeyEvents(e){e.addEventListener("keydown",this._onKeyDown),this._domElementKeyEvents=e}stopListenToKeyEvents(){this._domElementKeyEvents!==null&&(this._domElementKeyEvents.removeEventListener("keydown",this._onKeyDown),this._domElementKeyEvents=null)}saveState(){this.target0.copy(this.target),this.position0.copy(this.object.position),this.zoom0=this.object.zoom}reset(){this.target.copy(this.target0),this.object.position.copy(this.position0),this.object.zoom=this.zoom0,this.object.updateProjectionMatrix(),this.dispatchEvent($d),this.update(),this.state=Et.NONE}pan(e,t){this._pan(e,t),this.update()}dollyIn(e){this._dollyIn(e),this.update()}dollyOut(e){this._dollyOut(e),this.update()}rotateLeft(e){this._rotateLeft(e),this.update()}rotateUp(e){this._rotateUp(e),this.update()}update(e=null){let t=this.object.position;cn.copy(t).sub(this.target),cn.applyQuaternion(this._quat),this._spherical.setFromVector3(cn),this.autoRotate&&this.state===Et.NONE&&this._rotateLeft(this._getAutoRotationAngle(e)),this.enableDamping?(this._spherical.theta+=this._sphericalDelta.theta*this.dampingFactor,this._spherical.phi+=this._sphericalDelta.phi*this.dampingFactor):(this._spherical.theta+=this._sphericalDelta.theta,this._spherical.phi+=this._sphericalDelta.phi);let i=this.minAzimuthAngle,r=this.maxAzimuthAngle;isFinite(i)&&isFinite(r)&&(i<-Math.PI?i+=kn:i>Math.PI&&(i-=kn),r<-Math.PI?r+=kn:r>Math.PI&&(r-=kn),i<=r?this._spherical.theta=Math.max(i,Math.min(r,this._spherical.theta)):this._spherical.theta=this._spherical.theta>(i+r)/2?Math.max(i,this._spherical.theta):Math.min(r,this._spherical.theta)),this._spherical.phi=Math.max(this.minPolarAngle,Math.min(this.maxPolarAngle,this._spherical.phi)),this._spherical.makeSafe(),this.enableDamping===!0?this.target.addScaledVector(this._panOffset,this.dampingFactor):this.target.add(this._panOffset),this.target.sub(this.cursor),this.target.clampLength(this.minTargetRadius,this.maxTargetRadius),this.target.add(this.cursor);let s=!1;if(this.zoomToCursor&&this._performCursorZoom||this.object.isOrthographicCamera)this._spherical.radius=this._clampDistance(this._spherical.radius);else{let o=this._spherical.radius;this._spherical.radius=this._clampDistance(this._spherical.radius*this._scale),s=o!=this._spherical.radius}if(cn.setFromSpherical(this._spherical),cn.applyQuaternion(this._quatInverse),t.copy(this.target).add(cn),this.object.lookAt(this.target),this.enableDamping===!0?(this._sphericalDelta.theta*=1-this.dampingFactor,this._sphericalDelta.phi*=1-this.dampingFactor,this._panOffset.multiplyScalar(1-this.dampingFactor)):(this._sphericalDelta.set(0,0,0),this._panOffset.set(0,0,0)),this.zoomToCursor&&this._performCursorZoom){let o=null;if(this.object.isPerspectiveCamera){let a=cn.length();o=this._clampDistance(a*this._scale);let c=a-o;this.object.position.addScaledVector(this._dollyDirection,c),this.object.updateMatrixWorld(),s=!!c}else if(this.object.isOrthographicCamera){let a=new V(this._mouse.x,this._mouse.y,0);a.unproject(this.object);let c=this.object.zoom;this.object.zoom=Math.max(this.minZoom,Math.min(this.maxZoom,this.object.zoom/this._scale)),this.object.updateProjectionMatrix(),s=c!==this.object.zoom;let l=new V(this._mouse.x,this._mouse.y,0);l.unproject(this.object),this.object.position.sub(l).add(a),this.object.updateMatrixWorld(),o=cn.length()}else console.warn("WARNING: OrbitControls.js encountered an unknown camera type - zoom to cursor disabled."),this.zoomToCursor=!1;o!==null&&(this.screenSpacePanning?this.target.set(0,0,-1).transformDirection(this.object.matrix).multiplyScalar(o).add(this.object.position):(Al.origin.copy(this.object.position),Al.direction.set(0,0,-1).transformDirection(this.object.matrix),Math.abs(this.object.up.dot(Al.direction))<Qv?this.object.lookAt(this.target):(Yd.setFromNormalAndCoplanarPoint(this.object.up,this.target),Al.intersectPlane(Yd,this.target))))}else if(this.object.isOrthographicCamera){let o=this.object.zoom;this.object.zoom=Math.max(this.minZoom,Math.min(this.maxZoom,this.object.zoom/this._scale)),o!==this.object.zoom&&(this.object.updateProjectionMatrix(),s=!0)}return this._scale=1,this._performCursorZoom=!1,s||this._lastPosition.distanceToSquared(this.object.position)>xu||8*(1-this._lastQuaternion.dot(this.object.quaternion))>xu||this._lastTargetPosition.distanceToSquared(this.target)>xu?(this.dispatchEvent($d),this._lastPosition.copy(this.object.position),this._lastQuaternion.copy(this.object.quaternion),this._lastTargetPosition.copy(this.target),!0):!1}_getAutoRotationAngle(e){return e!==null?kn/60*this.autoRotateSpeed*e:kn/60/60*this.autoRotateSpeed}_getZoomScale(e){let t=Math.abs(e*.01);return Math.pow(.95,this.zoomSpeed*t)}_rotateLeft(e){this._sphericalDelta.theta-=e}_rotateUp(e){this._sphericalDelta.phi-=e}_panLeft(e,t){cn.setFromMatrixColumn(t,0),cn.multiplyScalar(-e),this._panOffset.add(cn)}_panUp(e,t){this.screenSpacePanning===!0?cn.setFromMatrixColumn(t,1):(cn.setFromMatrixColumn(t,0),cn.crossVectors(this.object.up,cn)),cn.multiplyScalar(e),this._panOffset.add(cn)}_pan(e,t){let i=this.domElement;if(this.object.isPerspectiveCamera){let r=this.object.position;cn.copy(r).sub(this.target);let s=cn.length();s*=Math.tan(this.object.fov/2*Math.PI/180),this._panLeft(2*e*s/i.clientHeight,this.object.matrix),this._panUp(2*t*s/i.clientHeight,this.object.matrix)}else this.object.isOrthographicCamera?(this._panLeft(e*(this.object.right-this.object.left)/this.object.zoom/i.clientWidth,this.object.matrix),this._panUp(t*(this.object.top-this.object.bottom)/this.object.zoom/i.clientHeight,this.object.matrix)):(console.warn("WARNING: OrbitControls.js encountered an unknown camera type - pan disabled."),this.enablePan=!1)}_dollyOut(e){this.object.isPerspectiveCamera||this.object.isOrthographicCamera?this._scale/=e:(console.warn("WARNING: OrbitControls.js encountered an unknown camera type - dolly/zoom disabled."),this.enableZoom=!1)}_dollyIn(e){this.object.isPerspectiveCamera||this.object.isOrthographicCamera?this._scale*=e:(console.warn("WARNING: OrbitControls.js encountered an unknown camera type - dolly/zoom disabled."),this.enableZoom=!1)}_updateZoomParameters(e,t){if(!this.zoomToCursor)return;this._performCursorZoom=!0;let i=this.domElement.getBoundingClientRect(),r=e-i.left,s=t-i.top,o=i.width,a=i.height;this._mouse.x=r/o*2-1,this._mouse.y=-(s/a)*2+1,this._dollyDirection.set(this._mouse.x,this._mouse.y,1).unproject(this.object).sub(this.object.position).normalize()}_clampDistance(e){return Math.max(this.minDistance,Math.min(this.maxDistance,e))}_handleMouseDownRotate(e){this._rotateStart.set(e.clientX,e.clientY)}_handleMouseDownDolly(e){this._updateZoomParameters(e.clientX,e.clientX),this._dollyStart.set(e.clientX,e.clientY)}_handleMouseDownPan(e){this._panStart.set(e.clientX,e.clientY)}_handleMouseMoveRotate(e){this._rotateEnd.set(e.clientX,e.clientY),this._rotateDelta.subVectors(this._rotateEnd,this._rotateStart).multiplyScalar(this.rotateSpeed);let t=this.domElement;this._rotateLeft(kn*this._rotateDelta.x/t.clientHeight),this._rotateUp(kn*this._rotateDelta.y/t.clientHeight),this._rotateStart.copy(this._rotateEnd),this.update()}_handleMouseMoveDolly(e){this._dollyEnd.set(e.clientX,e.clientY),this._dollyDelta.subVectors(this._dollyEnd,this._dollyStart),this._dollyDelta.y>0?this._dollyOut(this._getZoomScale(this._dollyDelta.y)):this._dollyDelta.y<0&&this._dollyIn(this._getZoomScale(this._dollyDelta.y)),this._dollyStart.copy(this._dollyEnd),this.update()}_handleMouseMovePan(e){this._panEnd.set(e.clientX,e.clientY),this._panDelta.subVectors(this._panEnd,this._panStart).multiplyScalar(this.panSpeed),this._pan(this._panDelta.x,this._panDelta.y),this._panStart.copy(this._panEnd),this.update()}_handleMouseWheel(e){this._updateZoomParameters(e.clientX,e.clientY),e.deltaY<0?this._dollyIn(this._getZoomScale(e.deltaY)):e.deltaY>0&&this._dollyOut(this._getZoomScale(e.deltaY)),this.update()}_handleKeyDown(e){let t=!1;switch(e.code){case this.keys.UP:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateUp(kn*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(0,this.keyPanSpeed),t=!0;break;case this.keys.BOTTOM:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateUp(-kn*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(0,-this.keyPanSpeed),t=!0;break;case this.keys.LEFT:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateLeft(kn*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(this.keyPanSpeed,0),t=!0;break;case this.keys.RIGHT:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateLeft(-kn*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(-this.keyPanSpeed,0),t=!0;break}t&&(e.preventDefault(),this.update())}_handleTouchStartRotate(e){if(this._pointers.length===1)this._rotateStart.set(e.pageX,e.pageY);else{let t=this._getSecondPointerPosition(e),i=.5*(e.pageX+t.x),r=.5*(e.pageY+t.y);this._rotateStart.set(i,r)}}_handleTouchStartPan(e){if(this._pointers.length===1)this._panStart.set(e.pageX,e.pageY);else{let t=this._getSecondPointerPosition(e),i=.5*(e.pageX+t.x),r=.5*(e.pageY+t.y);this._panStart.set(i,r)}}_handleTouchStartDolly(e){let t=this._getSecondPointerPosition(e),i=e.pageX-t.x,r=e.pageY-t.y,s=Math.sqrt(i*i+r*r);this._dollyStart.set(0,s)}_handleTouchStartDollyPan(e){this.enableZoom&&this._handleTouchStartDolly(e),this.enablePan&&this._handleTouchStartPan(e)}_handleTouchStartDollyRotate(e){this.enableZoom&&this._handleTouchStartDolly(e),this.enableRotate&&this._handleTouchStartRotate(e)}_handleTouchMoveRotate(e){if(this._pointers.length==1)this._rotateEnd.set(e.pageX,e.pageY);else{let i=this._getSecondPointerPosition(e),r=.5*(e.pageX+i.x),s=.5*(e.pageY+i.y);this._rotateEnd.set(r,s)}this._rotateDelta.subVectors(this._rotateEnd,this._rotateStart).multiplyScalar(this.rotateSpeed);let t=this.domElement;this._rotateLeft(kn*this._rotateDelta.x/t.clientHeight),this._rotateUp(kn*this._rotateDelta.y/t.clientHeight),this._rotateStart.copy(this._rotateEnd)}_handleTouchMovePan(e){if(this._pointers.length===1)this._panEnd.set(e.pageX,e.pageY);else{let t=this._getSecondPointerPosition(e),i=.5*(e.pageX+t.x),r=.5*(e.pageY+t.y);this._panEnd.set(i,r)}this._panDelta.subVectors(this._panEnd,this._panStart).multiplyScalar(this.panSpeed),this._pan(this._panDelta.x,this._panDelta.y),this._panStart.copy(this._panEnd)}_handleTouchMoveDolly(e){let t=this._getSecondPointerPosition(e),i=e.pageX-t.x,r=e.pageY-t.y,s=Math.sqrt(i*i+r*r);this._dollyEnd.set(0,s),this._dollyDelta.set(0,Math.pow(this._dollyEnd.y/this._dollyStart.y,this.zoomSpeed)),this._dollyOut(this._dollyDelta.y),this._dollyStart.copy(this._dollyEnd);let o=(e.pageX+t.x)*.5,a=(e.pageY+t.y)*.5;this._updateZoomParameters(o,a)}_handleTouchMoveDollyPan(e){this.enableZoom&&this._handleTouchMoveDolly(e),this.enablePan&&this._handleTouchMovePan(e)}_handleTouchMoveDollyRotate(e){this.enableZoom&&this._handleTouchMoveDolly(e),this.enableRotate&&this._handleTouchMoveRotate(e)}_addPointer(e){this._pointers.push(e.pointerId)}_removePointer(e){delete this._pointerPositions[e.pointerId];for(let t=0;t<this._pointers.length;t++)if(this._pointers[t]==e.pointerId){this._pointers.splice(t,1);return}}_isTrackingPointer(e){for(let t=0;t<this._pointers.length;t++)if(this._pointers[t]==e.pointerId)return!0;return!1}_trackPointer(e){let t=this._pointerPositions[e.pointerId];t===void 0&&(t=new He,this._pointerPositions[e.pointerId]=t),t.set(e.pageX,e.pageY)}_getSecondPointerPosition(e){let t=e.pointerId===this._pointers[0]?this._pointers[1]:this._pointers[0];return this._pointerPositions[t]}_customWheelEvent(e){let t=e.deltaMode,i={clientX:e.clientX,clientY:e.clientY,deltaY:e.deltaY};switch(t){case 1:i.deltaY*=16;break;case 2:i.deltaY*=100;break}return e.ctrlKey&&!this._controlActive&&(i.deltaY*=10),i}};function e_(n){this.enabled!==!1&&(this._pointers.length===0&&(this.domElement.setPointerCapture(n.pointerId),this.domElement.ownerDocument.addEventListener("pointermove",this._onPointerMove),this.domElement.ownerDocument.addEventListener("pointerup",this._onPointerUp)),!this._isTrackingPointer(n)&&(this._addPointer(n),n.pointerType==="touch"?this._onTouchStart(n):this._onMouseDown(n),this._cursorStyle==="grab"&&(this.domElement.style.cursor="grabbing")))}function t_(n){this.enabled!==!1&&(n.pointerType==="touch"?this._onTouchMove(n):this._onMouseMove(n))}function n_(n){switch(this._removePointer(n),this._pointers.length){case 0:this.domElement.releasePointerCapture(n.pointerId),this.domElement.ownerDocument.removeEventListener("pointermove",this._onPointerMove),this.domElement.ownerDocument.removeEventListener("pointerup",this._onPointerUp),this.dispatchEvent(Zd),this.state=Et.NONE,this._cursorStyle==="grab"&&(this.domElement.style.cursor="grab");break;case 1:let e=this._pointers[0],t=this._pointerPositions[e];this._onTouchStart({pointerId:e,pageX:t.x,pageY:t.y});break}}function i_(n){let e;switch(n.button){case 0:e=this.mouseButtons.LEFT;break;case 1:e=this.mouseButtons.MIDDLE;break;case 2:e=this.mouseButtons.RIGHT;break;default:e=-1}switch(e){case $i.DOLLY:if(this.enableZoom===!1)return;this._handleMouseDownDolly(n),this.state=Et.DOLLY;break;case $i.ROTATE:if(n.ctrlKey||n.metaKey||n.shiftKey){if(this.enablePan===!1)return;this._handleMouseDownPan(n),this.state=Et.PAN}else{if(this.enableRotate===!1)return;this._handleMouseDownRotate(n),this.state=Et.ROTATE}break;case $i.PAN:if(n.ctrlKey||n.metaKey||n.shiftKey){if(this.enableRotate===!1)return;this._handleMouseDownRotate(n),this.state=Et.ROTATE}else{if(this.enablePan===!1)return;this._handleMouseDownPan(n),this.state=Et.PAN}break;default:this.state=Et.NONE}this.state!==Et.NONE&&this.dispatchEvent(vu)}function r_(n){switch(this.state){case Et.ROTATE:if(this.enableRotate===!1)return;this._handleMouseMoveRotate(n);break;case Et.DOLLY:if(this.enableZoom===!1)return;this._handleMouseMoveDolly(n);break;case Et.PAN:if(this.enablePan===!1)return;this._handleMouseMovePan(n);break}}function s_(n){this.enabled===!1||this.enableZoom===!1||this.state!==Et.NONE||(n.preventDefault(),this.dispatchEvent(vu),this._handleMouseWheel(this._customWheelEvent(n)),this.dispatchEvent(Zd))}function o_(n){this.enabled!==!1&&this._handleKeyDown(n)}function a_(n){switch(this._trackPointer(n),this._pointers.length){case 1:switch(this.touches.ONE){case Yi.ROTATE:if(this.enableRotate===!1)return;this._handleTouchStartRotate(n),this.state=Et.TOUCH_ROTATE;break;case Yi.PAN:if(this.enablePan===!1)return;this._handleTouchStartPan(n),this.state=Et.TOUCH_PAN;break;default:this.state=Et.NONE}break;case 2:switch(this.touches.TWO){case Yi.DOLLY_PAN:if(this.enableZoom===!1&&this.enablePan===!1)return;this._handleTouchStartDollyPan(n),this.state=Et.TOUCH_DOLLY_PAN;break;case Yi.DOLLY_ROTATE:if(this.enableZoom===!1&&this.enableRotate===!1)return;this._handleTouchStartDollyRotate(n),this.state=Et.TOUCH_DOLLY_ROTATE;break;default:this.state=Et.NONE}break;default:this.state=Et.NONE}this.state!==Et.NONE&&this.dispatchEvent(vu)}function l_(n){switch(this._trackPointer(n),this.state){case Et.TOUCH_ROTATE:if(this.enableRotate===!1)return;this._handleTouchMoveRotate(n),this.update();break;case Et.TOUCH_PAN:if(this.enablePan===!1)return;this._handleTouchMovePan(n),this.update();break;case Et.TOUCH_DOLLY_PAN:if(this.enableZoom===!1&&this.enablePan===!1)return;this._handleTouchMoveDollyPan(n),this.update();break;case Et.TOUCH_DOLLY_ROTATE:if(this.enableZoom===!1&&this.enableRotate===!1)return;this._handleTouchMoveDollyRotate(n),this.update();break;default:this.state=Et.NONE}}function c_(n){this.enabled!==!1&&n.preventDefault()}function u_(n){n.key==="Control"&&(this._controlActive=!0,this.domElement.getRootNode().addEventListener("keyup",this._interceptControlUp,{passive:!0,capture:!0}))}function h_(n){n.key==="Control"&&(this._controlActive=!1,this.domElement.getRootNode().removeEventListener("keyup",this._interceptControlUp,{passive:!0,capture:!0}))}var fo=new V;function Yn(n,e,t,i,r,s){let o=2*Math.PI*r/4,a=Math.max(s-2*r,0),c=Math.PI/4;fo.copy(e),fo[i]=0,fo.normalize();let l=.5*o/(o+a),h=1-fo.angleTo(n)/c;return Math.sign(fo[t])===1?h*l:a/(o+a)+l+l*(1-h)}var Pi=class n extends Ke{constructor(e=1,t=1,i=1,r=2,s=.1){let o=r*2+1;if(s=Math.min(e/2,t/2,i/2,s),super(1,1,1,o,o,o),this.type="RoundedBoxGeometry",this.parameters={width:e,height:t,depth:i,segments:r,radius:s},o===1)return;let a=this.toNonIndexed();this.index=null,this.attributes.position=a.attributes.position,this.attributes.normal=a.attributes.normal,this.attributes.uv=a.attributes.uv;let c=new V,l=new V,h=new V(e,t,i).divideScalar(2).subScalar(s),m=this.attributes.position.array,u=this.attributes.normal.array,g=this.attributes.uv.array,_=m.length/6,x=new V,f=.5/o;for(let d=0,S=0;d<m.length;d+=3,S+=2)switch(c.fromArray(m,d),l.copy(c),l.x-=Math.sign(l.x)*f,l.y-=Math.sign(l.y)*f,l.z-=Math.sign(l.z)*f,l.normalize(),m[d+0]=h.x*Math.sign(c.x)+l.x*s,m[d+1]=h.y*Math.sign(c.y)+l.y*s,m[d+2]=h.z*Math.sign(c.z)+l.z*s,u[d+0]=l.x,u[d+1]=l.y,u[d+2]=l.z,Math.floor(d/_)){case 0:x.set(1,0,0),g[S+0]=Yn(x,l,"z","y",s,i),g[S+1]=1-Yn(x,l,"y","z",s,t);break;case 1:x.set(-1,0,0),g[S+0]=1-Yn(x,l,"z","y",s,i),g[S+1]=1-Yn(x,l,"y","z",s,t);break;case 2:x.set(0,1,0),g[S+0]=1-Yn(x,l,"x","z",s,e),g[S+1]=Yn(x,l,"z","x",s,i);break;case 3:x.set(0,-1,0),g[S+0]=1-Yn(x,l,"x","z",s,e),g[S+1]=1-Yn(x,l,"z","x",s,i);break;case 4:x.set(0,0,1),g[S+0]=1-Yn(x,l,"x","y",s,e),g[S+1]=1-Yn(x,l,"y","x",s,t);break;case 5:x.set(0,0,-1),g[S+0]=Yn(x,l,"x","y",s,e),g[S+1]=1-Yn(x,l,"y","x",s,t);break}}static fromJSON(e){return new n(e.width,e.height,e.depth,e.segments,e.radius)}};var po=.46,d_=.5,f_=.78,p_=.82,mo=.24,m_=1.25,g_=.35;function Kd(n,e,t){let i=new _t;i.name=e?"robot-orchestrator":"robot",e&&i.scale.setScalar(m_);let r=[],s=[],o=(pe,me,qe=.1)=>{let Le=new St({color:pe,roughness:me,metalness:qe});return r.push(Le),s.push(Le),Le},a=o(t.sheet,.65),c=o(t.paper,.6),l=o(new ze().setHSL(n/360,.45,.55),.5),h=o(t.rule,.45),m=o(t.ink,.8,.05),u=o(t.signal,.5),g=new St({color:t.ink,roughness:.12,metalness:.4}),_=new St({color:t.ok,emissive:t.ok,emissiveIntensity:1.2,roughness:.3}),x=new St({color:t.signal,emissive:t.signal,emissiveIntensity:1.2,roughness:.4});r.push(g,_,x);let f=(pe,me,qe,Le=0,Je=0,se=0)=>{let le=new xt(me,qe);return le.position.set(Le,Je,se),le.castShadow=!0,le.receiveShadow=!0,pe.add(le),le},d=new _t;i.add(d);let S=new Pi(.16,.22,.86,3,.07);f(d,S,m,-.36,.11,0),f(d,S,m,.36,.11,0);let P=new Dt(.08,.08,.03,16);P.rotateZ(Math.PI/2);let w=[];for(let pe of[-1,1])for(let me of[-.28,0,.28])w.push(f(d,P,h,pe*.445,.11,me));let T=new _t;i.add(T),f(T,new Pi(.62,d_,.6,4,.12),a,0,po,0),f(T,new Pi(.36,.26,.03,2,.01),l,0,po,.315),f(T,new Ke(.28,.012,.012),h,0,po,.33);let A=new Jt(.018,8,6);for(let pe of[-.15,.15])for(let me of[po-.1,po+.1])f(T,A,h,pe,me,.33);f(T,new Dt(.07,.08,.14,12),m,0,f_,0);let D=new Dt(.045,.04,.3,12);D.translate(0,-.15,0);let p=new Pi(.1,.05,.08,2,.02),M=new Ke(.025,.08,.03),y=pe=>{let me=new _t;me.position.set(pe*.34,.6,0),T.add(me),f(me,D,a);let qe=new _t;return qe.position.set(0,-.3,0),me.add(qe),f(qe,p,m),f(qe,M,m,-.035,-.05,0),f(qe,M,m,.035,-.05,0),me},C=y(-1),k=y(1),L=new _t;L.position.set(0,p_,0),T.add(L),f(L,new Pi(.62,mo,.42,3,.09),c,0,mo/2,0);let U=new Dt(.13,.13,.18,24);U.rotateX(Math.PI/2);let F=new pi(.1,24),H=new pi(.07,24),q=[];for(let pe of[-.16,.16])f(L,U,c,pe,.12,.2),f(L,F,g,pe,.12,.292),q.push(f(L,H,_,pe,.12,.296));let ee;e&&(f(L,new Dt(.01,.01,.16,6),m,.22,mo+.08,-.05),ee=f(L,new Jt(.03,10,8),x,.22,mo+.17,-.05),f(T,new Pi(.36,.03,.02,2,.008),u,0,.62,.31));let J="idle",ne=1.2,ce=pe=>{J=pe;let me=pe==="offline"?t.inkMuted:pe==="working"?t.info:t.ok;_.color.set(me),_.emissive.set(me),ne=pe==="offline"?.2:pe==="working"?2:1.3};return ce("idle"),{group:i,labelAnchor(pe){return L.updateWorldMatrix(!0,!1),L.localToWorld(pe.set(0,mo+g_,0))},setState(pe){pe!==J&&ce(pe)},update(pe,me){let qe=Math.min(1,pe*8),Le=J,Je=0,se=0;Le==="idle"?(Je=.03*Math.sin(me*.5),se=.12*Math.sin(me*.9)):Le==="working"?(Je=.14+.08*Math.sin(me*5),se=.04*Math.sin(me*1.3)):Le==="walking"?(Je=.04,se=.06*Math.sin(me*3)):(Je=.5,se=.12),L.rotation.x+=(Je-L.rotation.x)*qe,L.rotation.z+=(se-L.rotation.z)*qe;let le=0,Se=0;if(Le==="idle"?le=.012*Math.sin(me*1.6):Le==="working"?le=.004*Math.sin(me*2.5):Le==="walking"?(le=.01*Math.abs(Math.sin(me*6)),Se=.05*Math.sin(me*6)):le=-.02,T.position.y=le,T.rotation.z=Se,d.position.y=Le==="walking"?.012*Math.abs(Math.sin(me*10)):0,Le==="walking")for(let ye of w)ye.rotation.x+=pe*8;if(Le==="working"){let ye=Math.sin(me*11);C.rotation.x=-.9+.2*ye,k.rotation.x=-.9-.2*ye}else if(Le==="walking"){let ye=.25*Math.sin(me*6);C.rotation.x=ye,k.rotation.x=-ye}else if(Le==="idle"){let ye=.05*Math.sin(me*1.1);C.rotation.x=ye,k.rotation.x=-ye}else C.rotation.x=.2,k.rotation.x=.2;let ke=1;Le==="idle"&&me%3.6<.14?ke=.1:Le==="offline"&&(ke=.45);for(let ye of q)ye.scale.y=ke;_.emissiveIntensity=Le==="working"?ne+.3*Math.sin(me*5):ne,ee&&(x.emissiveIntensity=1.1+.5*Math.sin(me*3))},setHighlighted(pe){for(let me of s)me.emissive.set(pe?t.signal:0),me.emissiveIntensity=pe?.35:0},dispose(){i.traverse(pe=>{pe instanceof xt&&pe.geometry.dispose()});for(let pe of r)pe.dispose()}}}var Jd=new Map,jd=new Map;function _u(n){let e=n>>>0;return()=>{e=e+1831565813>>>0;let t=e;return t=Math.imul(t^t>>>15,t|1),t^=t+Math.imul(t^t>>>7,t|61),((t^t>>>14)>>>0)/4294967296}}function x_(n){let e=2166136261;for(let t=0;t<n.length;t++)e=Math.imul(e^n.charCodeAt(t),16777619);return e>>>0}function vi(n,e,t,i,r){let s=Jd.get(n);if(s)return s;let o=document.createElement("canvas");o.width=e,o.height=t;let a=o.getContext("2d");a&&r(a,e,t,_u(x_(n)));let c=new dr(o);return c.wrapS=ur,c.wrapT=ur,c.repeat.set(i,i),c.colorSpace=Ht,c.anisotropy=4,Jd.set(n,c),c}var vr=n=>n.getStyle();function Qd(){return vi("grass",256,256,50,(n,e,t,i)=>{n.fillStyle="#7a9d5c",n.fillRect(0,0,e,t);for(let r=0;r<2200;r++)n.fillStyle=`hsla(${80+i()*50}, ${35+i()*25}%, ${30+i()*26}%, ${.35+i()*.4})`,n.fillRect(i()*e,i()*t,1+i()*2.2,(1+i()*2.2)*(1+i()));for(let r=0;r<40;r++)n.fillStyle=`hsla(${50+i()*20}, 40%, 55%, 0.25)`,n.beginPath(),n.arc(i()*e,i()*t,4+i()*10,0,Math.PI*2),n.fill()})}function yu(n,e=4){return vi(`paving:${vr(n)}:${e}`,256,256,e,(t,i,r,s)=>{let a=r/4,c=i/3;t.fillStyle=vr(n.clone().multiplyScalar(.7)),t.fillRect(0,0,i,r);for(let l=0;l<4;l++){let h=l%2*(c/2);for(let m=-1;m<=3;m++)t.fillStyle=vr(n.clone().multiplyScalar(.88+s()*.16)),t.fillRect(m*c+h+2,l*a+2,c-4,a-4)}for(let l=0;l<160;l++)t.fillStyle=`rgba(40,34,28,${.05+s()*.1})`,t.fillRect(s()*i,s()*r,1+s()*2,1+s()*2)})}function ef(n,e=3){return vi(`tiles:${Math.round(n)}:${e}`,256,256,e,(t,i,r,s)=>{let o=new ze().setHSL(n/360,.18,.74,Ht),a=new ze().setHSL(n/360,.3,.86,Ht);t.fillStyle=vr(o),t.fillRect(0,0,i,r);let c=4,l=i/c;for(let h=0;h<c;h++)for(let m=0;m<c;m++)t.fillStyle=vr(a.clone().multiplyScalar(.96+s()*.08)),t.fillRect(m*l+2,h*l+2,l-4,l-4)})}function go(){return vi("wood",256,256,1,(n,e,t,i)=>{let s=t/4;for(let o=0;o<4;o++){n.fillStyle=`hsl(${26+i()*6}, ${40+i()*12}%, ${40+i()*10}%)`,n.fillRect(0,o*s,e,s);for(let a=0;a<14;a++){let c=o*s+i()*s;n.strokeStyle=`hsla(26, 45%, 25%, ${.15+i()*.2})`,n.lineWidth=1+i(),n.beginPath(),n.moveTo(0,c),n.bezierCurveTo(e*.3,c+(i()-.5)*6,e*.7,c+(i()-.5)*6,e,c),n.stroke()}n.fillStyle="rgba(40,25,12,0.55)",n.fillRect(0,o*s,e,2)}})}function bu(){return vi("metal",128,128,2,(n,e,t,i)=>{n.fillStyle="#9aa0a8",n.fillRect(0,0,e,t);for(let r=0;r<260;r++){n.strokeStyle=i()<.5?"rgba(255,255,255,0.18)":"rgba(40,44,52,0.18)";let s=i()*t;n.beginPath(),n.moveTo(0,s),n.lineTo(e,s+(i()-.5)*1.5),n.stroke()}})}function Cl(n){return vi(`polish:${vr(n)}`,256,256,2,(e,t,i,r)=>{e.fillStyle=vr(n),e.fillRect(0,0,t,i);for(let s=0;s<26;s++){e.strokeStyle=`rgba(90,80,70,${.05+r()*.08})`,e.lineWidth=.6+r()*1.4;let o=r()*t,a=r()*i;e.beginPath(),e.moveTo(o,a);for(let c=0;c<4;c++)o+=(r()-.5)*90,a+=(r()-.5)*60,e.lineTo(o,a);e.stroke()}})}function tf(){return vi("water",256,256,2,(n,e,t,i)=>{let r=n.createLinearGradient(0,0,0,t);r.addColorStop(0,"#7fd3da"),r.addColorStop(1,"#2f8fa6"),n.fillStyle=r,n.fillRect(0,0,e,t);for(let s=0;s<60;s++){let o=i()*e,a=i()*t;n.strokeStyle=`rgba(255,255,255,${.12+i()*.25})`,n.lineWidth=1+i()*2,n.beginPath(),n.moveTo(o,a),n.quadraticCurveTo(o+12+i()*12,a-4,o+28+i()*20,a),n.stroke()}})}function Mu(n){let e=`side:${Math.round(n)}`,t=jd.get(e);if(t)return t;let i=new St({color:new ze().setHSL(n/360,.25,.68,Ht),roughness:.95});return jd.set(e,i),i}var v_=new Set(["builder","qa","release","security"]),__=new Set(["researcher","librarian","planner"]),y_=new Set(["architect","reviewer"]),wu=new ze(16766874),sf=new ze(13208422);function Vt(n,e,t=.1){return new St({color:n,roughness:e,metalness:t})}function oi(n,e,t){let i=new ze().setHSL(e/360,.45,.6,Ht);return n.clone().lerp(i,t)}function Ve(n,e,t,i,r,s){let o=new xt(e,t);return o.position.set(i,r,s),n.add(o),o}function yr(n){n.traverse(e=>{e instanceof xt&&(e.castShadow=!0,e.receiveShadow=!0)})}function _r(n){n.updateWorldMatrix(!0,!1);let e=new Xn().setFromObject(n);return{x0:e.min.x,x1:e.max.x,z0:e.min.z,z1:e.max.z}}function Su(n,e){return n[Math.floor(e()*n.length)]}function nf(n,e){return Math.abs(Math.atan2(Math.sin(n-e),Math.cos(n-e)))}function b_(n,e,t,i){let r=Math.max(t*t+i*i,1e-6),s=Math.max(0,Math.min(1,(n*t+e*i)/r));return Math.hypot(n-t*s,e-i*s)}function of(n){let e=new _t;return Ve(e,new Ke(1.3,.08,.42),n,0,.42,0),Ve(e,new Ke(.08,.4,.36),n,-.55,.2,0),Ve(e,new Ke(.08,.4,.36),n,.55,.2,0),Ve(e,new Ke(1.3,.4,.07),n,0,.66,-.2),e}function M_(n,e,t){let i=vi(`sign:${n}:${e.getStyle()}:${t.getStyle()}`,256,96,1,(r,s,o)=>{r.fillStyle=e.getStyle(),r.fillRect(0,0,s,o),r.strokeStyle=t.getStyle(),r.lineWidth=6,r.strokeRect(6,6,s-12,o-12),r.fillStyle=t.getStyle(),r.font="600 30px ui-monospace, SFMono-Regular, Menlo, monospace",r.textAlign="center",r.textBaseline="middle",r.fillText(n,s/2,o/2+2)});return new St({map:i,roughness:.6})}function S_(n,e,t,i,r,s){let o=new _t;o.position.set(e,0,t),Ve(o,new Ke(1,.06,.55),i,0,.55,0),Ve(o,new Ke(.08,.55,.5),i,-.42,.275,0),Ve(o,new Ke(.08,.55,.5),i,.42,.275,0),Ve(o,new Ke(.3,.02,.18),r,0,.59,-.12),Ve(o,new Ke(.07,.2,.05),r,0,.7,-.12),Ve(o,new Ke(.62,.38,.04),s,0,.95,-.14),n.add(o)}function rf(n,e,t,i,r,s,o){Ve(n,new Dt(.22*i,.16*i,.3*i,14),r,e,.15*i,t),Ve(n,new Jt(.34*i,14,10),s,e,.5*i,t),Ve(n,new ns(.22*i,.45*i,10),o,e,.85*i,t)}function af(n,e,t,i){let r=new _t,s=e,o=Vt(oi(i.rule,n,.3),.8,0),a=Vt(oi(i.ink3,n,.2),.6,.2),c=Vt(oi(i.ink3,n,.1),.45,.4),l=Vt(oi(sf,n,.1),.9,0),h=Vt(oi(i.ok,n,.2),.85,0),m=Vt(oi(i.ok,n,.45),.85,0),u=Vt(oi(i.rule,n,.55),.9,0),g=new St({color:oi(i.ink3,n,.3),emissive:i.info,emissiveIntensity:.05,roughness:.35}),_=new on({color:i.ok,toneMapped:!1}),x=new on({color:i.info,toneMapped:!1}),f=s>=3?3:2;for(let M=0;M<f;M++)S_(r,-s+1.2+M*(2*s-2.4)/(f-1),s-.9,o,c,g);rf(r,s-.5,-s+.55,1,l,h,m),s>=3&&rf(r,-s+.5,-s+.55,.9,l,h,m),Ve(r,new Ke(.5,.5,.5),u,s-.5,.25,.05),Ve(r,new Ke(.45,.45,.45),u,s-.45,.725,.1);for(let M of[s-1.6,-s+1.6])Ve(r,new Ke(.3,.4,1.2),h,M,.2,0);let d=-s+.4,S=s-.4;Ve(r,new Dt(.035,.035,1.9,8),c,d,.95,S);let P=new on({color:wu,toneMapped:!1});Ve(r,new Jt(.13,12,8),P,d,1.95,S);let w=new Zs(wu,.35,4.5,2);w.position.set(d,1.8,S),r.add(w);let T=-s+.5;if(v_.has(t))for(let M of[-.55,.2]){Ve(r,new Ke(.5,1.5,.45),a,T,.75,M);for(let y of[.3,.6,.9,1.2])for(let C of[-.12,.12])Ve(r,new Ke(.02,.05,.05),y+C>.7?_:x,T+.26,y,M+C)}else if(__.has(t)){let M=[oi(i.signal,n,.3),oi(i.info,n,.2),oi(i.ok,n,.1)].map(y=>Vt(y,.7,0));Ve(r,new Ke(.5,1.5,1),a,T,.75,-.2);for(let y of[.35,.75,1.15]){Ve(r,new Ke(.56,.04,1),o,T,y,-.2);for(let C=0;C<4;C++)Ve(r,new Ke(.02,.22,.12),M[C%M.length]??o,T+.265,y+.13,-.6+C*.28)}}else if(y_.has(t)){let M=Vt(i.sheet,.5,0);Ve(r,new Ke(.08,1,1.5),c,-s+.1,1.05,0),Ve(r,new Ke(.02,.8,1.3),M,-s+.15,1.05,0),Ve(r,new Ke(.02,.12,.4),Vt(i.signal,.6,0),-s+.17,1.2,-.25),Ve(r,new Ke(.02,.1,.5),Vt(i.info,.6,0),-s+.17,.9,.3)}else Ve(r,new Ke(.4,.35,1.3),l,T,.175,-.1),Ve(r,new Jt(.3,12,8),h,T,.55,-.5),Ve(r,new Jt(.3,12,8),m,T,.55,.3);let A=M_(t.toUpperCase(),i.sheet,i.ink3),D=new _t;D.position.set(0,0,-s+1.5),Ve(D,new Dt(.035,.035,1.2,8),c,0,.6,0);let p=new xt(new Ke(.9,.34,.04),[o,o,o,o,A,A]);return p.position.set(0,1.05,0),D.add(p),r.add(D),r.userData={screens:[g],animate:M=>{_.color.copy(i.ok).multiplyScalar(.35+.65*Math.max(0,Math.sin(M*2.7))),x.color.copy(i.info).multiplyScalar(.35+.65*Math.max(0,Math.sin(M*3.9+1))),w.intensity=.35*(1+.03*Math.sin(M*7.1)+.02*Math.sin(M*2.3))}},yr(r),r}function lf(n){let e=new _t,t=n.rule.clone().lerp(n.sheet,.4),i=Ve(e,new Dt(4.3,4.45,.06,64),new St({map:yu(t,4),roughness:.9}),0,.03,0);i.receiveShadow=!0;let r=Ve(e,new Ws(3.7,3.95,72),Vt(t.clone().multiplyScalar(.72),.9,0),0,.066,0);r.rotation.x=-Math.PI/2;let s=Ve(e,new fr(3.3,.025,6,72),Vt(n.rule,.8,0),0,.065,0);s.rotation.x=Math.PI/2;let o=new St({map:go(),color:n.rule.clone().lerp(n.ink3,.25),roughness:.8});for(let l=0;l<4;l++){let h=l/4*Math.PI*2,m=Math.cos(h)*3.6,u=Math.sin(h)*3.6,g=of(o);g.position.set(m,0,u),g.rotation.y=Math.atan2(-m,-u),e.add(g)}let a=Ve(e,new Dt(.05,.05,1.35,8),Vt(n.ink3,.45,.4),0,1.475,0);a.castShadow=!0;let c=new xt(new Jt(.75,24,16),new St({color:n.signal,emissive:n.signal,emissiveIntensity:.7,wireframe:!0,transparent:!0,opacity:.5,depthWrite:!1}));return c.position.set(0,2.9,0),e.add(c),e.userData={animate:l=>{c.rotation.y=l*.4,c.position.y=2.9+Math.sin(l*.9)*.05}},yr(e),c.castShadow=!1,e}function cf(n,e){let t=new _t,i=[],r=4,s=[];for(let _=0;_<8;_++){let x=Math.PI/8+_*Math.PI/4;e.some(f=>nf(x,f)<.3)||s.every(f=>nf(x,f)>1)&&s.push(x)}let o=tf(),a=new St({map:Cl(n.sheet),roughness:.4}),c=new St({map:o,roughness:.12,transparent:!0,opacity:.9}),l=new St({map:go(),roughness:.8}),h=new St({map:bu(),roughness:.4,metalness:.5}),m=vi(`awning:${n.signal.getStyle()}:${n.sheet.getStyle()}`,256,64,1,(_,x,f)=>{for(let S=0;S<8;S++)_.fillStyle=S%2===1?n.signal.getStyle():n.sheet.getStyle(),_.fillRect(S*x/8,0,x/8,f)}),u=new St({map:m,roughness:.9,side:qn});return s.forEach((_,x)=>{let f=Math.cos(_)*r,d=Math.sin(_)*r,S=new _t;if(S.position.set(f,0,d),x===0){let P=Ve(S,new Dt(.62,.7,.26,32),a,0,.13,0),w=Ve(S,new pi(.55,32),c,0,.27,0);w.rotation.x=-Math.PI/2,Ve(S,new Dt(.05,.07,.6,8),a,0,.56,0),Ve(S,new Jt(.11,12,8),c,0,.92,0),i.push(_r(P))}else{S.rotation.y=Math.atan2(-f,-d);let P=Ve(S,new Ke(1.1,.72,.5),l,0,.36,0);Ve(S,new Ke(.26,.18,.26),Vt(n.ok,.7,0),.25,.81,0),Ve(S,new Ke(.24,.16,.24),Vt(n.signal,.7,0),-.3,.8,.05);for(let T of[-.5,.5])Ve(S,new Dt(.03,.03,2,6),h,T,1,.42);let w=Ve(S,new Ke(1.4,.04,.95),u,0,2,.1);w.rotation.x=.2,i.push(_r(P))}yr(S),t.add(S)}),{group:t,boxes:i,animate:_=>{o.offset.y=_*.04%1}}}function uf(n,e){let t=new _t,i=new St({map:yu(e.rule.clone().lerp(e.sheet,.35),2),roughness:.95});for(let r of n){let s=Math.hypot(r.x,r.z);if(s<.5)continue;let o=new xt(new Ke(s,.03,.9),i);o.position.set(r.x/2,.015,r.z/2),o.rotation.y=-Math.atan2(r.z,r.x),o.receiveShadow=!0,t.add(o)}return t}function hf(n,e){let t=new _t,i=[],r=_u(1177),s=0;for(let p of n)s=Math.max(s,Math.hypot(p.cx,p.cy)+p.r);let o=Math.min(34,s+6),a=new St({map:go(),roughness:.9}),c=[6262602,7840346,5077564,9416286].map(p=>Vt(new ze(p),.85,0)),l=[3104058,3960391].map(p=>Vt(new ze(p),.9,0)),h=[15907014,16245738].map(p=>Vt(new ze(p),.85,0)),m=[15909198,15305376,16249832,14243644,10190801].map(p=>Vt(new ze(p),.7,0)),u=Vt(e.rule.clone().lerp(e.ink3,.6),1,0),g=Vt(sf,.9,0),_=Vt(e.ink3.clone().lerp(e.sheet,.45),.95,0),x=new St({map:go(),roughness:.8}),f=new St({map:bu(),roughness:.4,metalness:.5}),d=new on({color:wu,toneMapped:!1}),S=(p,M,y)=>{if(Math.hypot(p,M)<6.4+y)return!1;for(let C of n)if(Math.abs(p-C.cx)<C.r+3+y&&Math.abs(M-C.cy)<C.r+3+y||b_(p,M,C.cx,C.cy)<1.4+y)return!1;for(let C of i)if(p+y>C.x0-.3&&p-y<C.x1+.3&&M+y>C.z0-.3&&M-y<C.z1+.3)return!1;return!0},P=(p,M)=>{let y=new _t;y.position.set(p,0,M);let C=Ve(y,new Dt(.035,.045,2.2,8),f,0,1.1,0);Ve(y,new Dt(.06,.08,.1,10),f,0,2.25,0),Ve(y,new Jt(.16,14,10),d,0,2.4,0),t.add(y),yr(y),i.push(_r(C))},w=(p,M)=>{let y=new _t;y.position.set(p,0,M);let C=.9+r()*.5,k=Ve(y,new Dt(.08,.12,C,8),a,0,C/2,0);if(r()<.35){let L=Su(l,r);for(let U=0;U<3;U++)Ve(y,new ns(.8-U*.2,.95,10),L,0,C+.25+U*.42,0)}else{let L=Su(r()<.2?h:c,r);Ve(y,new Jt(.72,14,10),L,0,C+.35,0),Ve(y,new Jt(.5,12,8),L,.28,C+.8,.12),Ve(y,new Jt(.42,12,8),L,-.22,C+.7,-.18)}t.add(y),yr(y),i.push(_r(k))},T=(p,M)=>{let y=new _t;y.position.set(p,0,M),y.rotation.y=r()*Math.PI;let C=Ve(y,new Ke(1.5,.2,.9),g,0,.1,0);Ve(y,new Ke(1.36,.06,.76),u,0,.19,0);for(let k=0;k<16;k++){let L=Su(m,r);Ve(y,new Jt(.06+r()*.03,8,6),L,(r()-.5)*1.1,.3+r()*.06,(r()-.5)*.6)}t.add(y),yr(y),i.push(_r(C))},A=(p,M)=>{let y=.3+r()*.35,C=new xt(new Vs(y,0),_);C.position.set(p,y*.4,M),C.rotation.set(r()*3,r()*3,r()*3),C.castShadow=!0,C.receiveShadow=!0,t.add(C),i.push(_r(C))},D=(p,M)=>{let y=of(x);y.position.set(p,0,M),y.rotation.y=r()*Math.PI*2,t.add(y),yr(y),i.push(_r(y))};for(let p of n){let M=Math.hypot(p.cx,p.cy);if(M<1)continue;let y=p.cx/M,C=p.cy/M;for(let k=6.6;k<M-p.r-3.2;k+=4.5)for(let L of[-1,1])P(y*k-C*L*1.2,C*k+y*L*1.2)}for(let p=0;p<140;p++){let M=(r()*2-1)*o,y=(r()*2-1)*o,C=r();C<.42?S(M,y,.7)&&w(M,y):C<.62?S(M,y,.85)&&T(M,y):C<.84?S(M,y,.4)&&A(M,y):S(M,y,.7)&&D(M,y)}return{group:t,boxes:i}}var w_=64,E_=32,T_=4e3,A_=1500,R_=1.6,jt=10,df=24,C_=60,ff=2,P_=.9,I_=[0,30,-30,60,-60];function Pl(n){return 2*(Math.max(1,Math.ceil(Math.sqrt(n.length)))+1)}function L_(n){let e=0;for(let t of n)e=e*31+t.charCodeAt(0)>>>0;return e%360}function pf(n,e,t=0){return[(n-e)*(w_/2),(n+e)*(E_/2)-t]}function k_(n){let e=new Map;for(let h of n){let m=h.tier==="orchestrator"?"":h.role;e.set(m,[...e.get(m)??[],h])}let t=[],i=new Map,r=(h,m)=>{t.push(h);let u=Math.max(1,Math.ceil(Math.sqrt(m.length)));m.sort((g,_)=>g.cid-_.cid).forEach((g,_)=>{let x=h.cx-(u-1)*2+_%u*4,f=h.cy-(u-1)*2+Math.floor(_/u)*4;i.set(g.cid,[h,x,f])})},s=e.get("");s&&r({role:"orchestrator",cx:0,cy:0,r:Pl(s),hue:25},s);let o=[...e.keys()].filter(h=>h).sort(),a=Math.max(0,...o.map(h=>Pl(e.get(h)))),c=s?Pl(s):0,l=Math.max(o.length>1?(2*a*Math.SQRT2+ff)/(2*Math.sin(Math.PI/o.length)):0,(c+a)*Math.SQRT2+ff);return o.forEach((h,m)=>{let u=e.get(h),g=m/o.length*Math.PI*2-Math.PI/2;r({role:h,cx:Math.round(Math.cos(g)*l),cy:Math.round(Math.sin(g)*l),r:Pl(u),hue:L_(h)},u)}),{districts:t,homes:i}}function D_(){let n=getComputedStyle(document.documentElement),e=(t,i)=>n.getPropertyValue(`--${t}`).trim()||i;return{paper:e("paper","#f5f3ee"),sheet:e("sheet","#ffffff"),rule:e("rule","#d6d2c8"),ink:e("ink","#1a1a1a"),ink2:e("ink-2","#4a4a4a"),ink3:e("ink-3","#8a8a8a"),signal:e("signal","#d9482b"),ok:e("ok","#2f8f5b"),info:e("info","#2f6fb0"),danger:e("danger","#c0392b")}}function Ii(n){return typeof n=="string"?n:""}function br(n,e,t){return Math.min(t,Math.max(e,n))}var N_=[[2.6,2.6],[-2.6,-2.6],[2.6,-2.6],[-2.6,2.6]],U_={builder:"rack",qa:"rack",release:"rack",security:"rack",researcher:"shelf",librarian:"shelf",planner:"shelf",architect:"board",reviewer:"board"};function F_(n){let e=N_.map(([t,i])=>({kind:"bench",x:t,y:i}));for(let t of n){if(t.role==="orchestrator")continue;let i=t.r-.7;e.push({kind:"plant",x:t.cx-i,y:t.cy-i,d:t},{kind:"desk",x:t.cx+i,y:t.cy-i,d:t},{kind:"desk",x:t.cx-i,y:t.cy+i,d:t},{kind:"lamp",x:t.cx+i,y:t.cy+i,d:t},{kind:"crate",x:t.cx-i,y:t.cy,d:t},{kind:U_[t.role]??"crate",x:t.cx,y:t.cy-i,d:t}),t.r>=4&&e.push({kind:"desk",x:t.cx+i,y:t.cy,d:t})}return e}function O_(n,e,t,i){return i.some(r=>r!==n&&Math.hypot(r.x-e,r.y-t)<P_)}function B_(n,e){let t=e.filter(i=>i!==n&&i.district===n.district&&i.agent.status==="idle"&&Math.hypot(i.x-n.x,i.y-n.y)<8);return t[Math.floor(Math.random()*t.length)]??null}function mf(n){let e=v("canvas.floor-canvas",{"aria-label":"The plaza floor"}),t=v("div.floor-card",{hidden:!0}),i=v("div.floor-legend",null,v("span",null,v("i.floor-key.working"),"Working"),v("span",null,v("i.floor-key.idle"),"Idle"),v("span",null,v("i.floor-key.offline"),"Offline")),r=v("div.floor-stage",null,e,t,i);n.classList.add("floor-view"),n.append(v("div.view-head",null,v("h1",null,"Floor"),v("div.actions",null,We("Reset view",()=>C(),{kind:"ghost",icon:"refresh"}))),r);let s=e.getContext("2d"),o=[],a=[],c=[],l=new Map,h=[],m={x:0,y:0},u=1,g=0,_=0,x=1,f=null,d=null,S="",P=null,w=!1,T=!0,A=0,D=performance.now(),p=D_();function M(R){let I=k_(R);o=I.districts;let O=[];for(let z of R){let X=I.homes.get(z.cid);if(!X)continue;let[Y,re,fe]=X,ge=l.get(z.cid);O.push(ge?Object.assign(ge,{agent:z,district:Y,home:[re,fe]}):{agent:z,district:Y,home:[re,fe],x:re,y:fe,tx:re,ty:fe,facing:1,walk:0,moving:!1,nextWander:performance.now()+Math.random()*3e3,bubble:null,activity:"idle",talk:null,talkUntil:0})}a=O,c=F_(o),l.clear(),a.forEach(z=>l.set(z.agent.cid,z)),d&&(d=l.get(d.agent.cid)??null),f&&(f=l.get(f.agent.cid)??null),W()}function y(R,I,O=0){let z=g/2+m.x,X=_/2+m.y,[Y,re]=pf(R,I,O);return[z+Y*u,X+re*u]}function C(){if(!o.length){m={x:0,y:0},u=1;return}let R=1/0,I=-1/0,O=1/0,z=-1/0;for(let re of o){let fe=[[re.cx-re.r,re.cy-re.r],[re.cx+re.r,re.cy-re.r],[re.cx+re.r,re.cy+re.r],[re.cx-re.r,re.cy+re.r]];for(let[ge,B]of fe){let[ue,oe]=pf(ge,B);R=Math.min(R,ue),I=Math.max(I,ue),O=Math.min(O,oe-60),z=Math.max(z,oe+jt)}}let X=Math.max(1,I-R),Y=Math.max(1,z-O);u=br(Math.min(g*.85/X,_*.85/Y),.4,2.5),m={x:-((R+I)/2)*u,y:-((O+z)/2)*u}}function k(R,I){for(let O of a){let z=O.agent.status==="idle";if(z||(O.talk=null),z&&I>=O.nextWander){let Ft=Math.random()<.3?B_(O,a):null;if(Ft)O.talk=Ft.agent.cid,O.talkUntil=I+9e3,O.nextWander=I+9e3;else{let wn=Math.max(.5,O.district.r-.7);O.tx=O.district.cx+(Math.random()*2-1)*wn,O.ty=O.district.cy+(Math.random()*2-1)*wn,O.nextWander=I+3e3+Math.random()*4e3}}let X=z&&O.talk!==null&&I<O.talkUntil?l.get(O.talk):void 0;O.talk!==null&&!X&&(O.talk=null);let[Y,re]=O.home,fe=X?[X.x+.9,X.y+.9]:z?[O.tx,O.ty]:[Y,re],[ge,B]=fe,ue=ge-O.x,oe=B-O.y,xe=Math.hypot(ue,oe),_e=Math.min(R_*R,xe),he=Math.atan2(oe,ue),we=I_.map(Ft=>he+Ft*Math.PI/180).find(Ft=>!O_(O,O.x+Math.cos(Ft)*_e,O.y+Math.sin(Ft)*_e,a));O.moving=xe>.02&&we!==void 0,we!==void 0&&(O.x+=Math.cos(we)*_e,O.y+=Math.sin(we)*_e);let{cx:Re,cy:vt,r:rt}=O.district;if(O.x=br(O.x,Re-rt+.5,Re+rt-.5),O.y=br(O.y,vt-rt+.5,vt+rt-.5),O.moving){O.walk+=R*9;let Ft=ue-oe;Math.abs(Ft)>.05&&(O.facing=Ft>0?1:-1)}X&&!O.moving&&(O.facing=X.x-O.x-(X.y-O.y)>=0?1:-1,O.bubble={text:"\xB7 \xB7 \xB7",until:I+3e3,tone:"info"},O.talk=null,O.nextWander=I+2500+Math.random()*2e3)}}function L(R,I){s.beginPath(),R.forEach(([O,z],X)=>X===0?s.moveTo(O,z):s.lineTo(O,z)),s.closePath(),s.fillStyle=I,s.fill()}function U(R,I,O,z,X){s.beginPath(),s.moveTo(R+X,I),s.arcTo(R+O,I,R+O,I+z,X),s.arcTo(R+O,I+z,R,I+z,X),s.arcTo(R,I+z,R,I,X),s.arcTo(R,I,R+O,I,X),s.closePath()}function F(R){let I=R.cx-R.r,O=R.cx+R.r,z=R.cy-R.r,X=R.cy+R.r,Y=(he,we)=>y(he,we,jt),re=(he,we)=>y(he,we,0);L([Y(O,z),Y(O,X),re(O,X),re(O,z)],`hsl(${R.hue} 30% 32% / 0.5)`),L([Y(O,X),Y(I,X),re(I,X),re(O,X)],`hsl(${R.hue} 30% 22% / 0.6)`);let fe=[Y(I,z),Y(O,z),Y(O,X),Y(I,X)];L(fe,p.sheet),L(fe,`hsl(${R.hue} 30% 50% / 0.12)`);let ge=R.role==="orchestrator";s.beginPath(),fe.forEach(([he,we],Re)=>Re===0?s.moveTo(he,we):s.lineTo(he,we)),s.closePath(),s.strokeStyle=ge?p.signal:p.rule,s.lineWidth=ge?2:1,s.stroke();let[B,ue]=Y(I,z),oe=R.role.toUpperCase();s.font="10px ui-monospace, SFMono-Regular, Menlo, monospace";let xe=s.measureText(oe).width+12,_e=ue+10;U(B-xe/2,_e,xe,16,3),s.fillStyle=p.sheet,s.fill(),s.strokeStyle=ge?p.signal:p.rule,s.lineWidth=1,s.stroke(),s.fillStyle=p.ink2,s.textAlign="center",s.textBaseline="middle",s.fillText(oe,B,_e+8.5)}function H(R,I,O){let Y=(xe,_e)=>y(xe,_e,jt+12),re=(xe,_e)=>y(xe,_e,jt),fe=[R-.32,I-.32],ge=[R+.32,I-.32],B=[R+.32,I+.32],ue=[R-.32,I+.32];L([Y(ge[0],ge[1]),Y(B[0],B[1]),re(B[0],B[1]),re(ge[0],ge[1])],p.ink3),L([Y(B[0],B[1]),Y(ue[0],ue[1]),re(ue[0],ue[1]),re(B[0],B[1])],p.ink2),L([Y(fe[0],fe[1]),Y(ge[0],ge[1]),Y(B[0],B[1]),Y(ue[0],ue[1])],p.sheet);let oe=[Y(R-.2,I-.2),Y(R+.2,I-.2),Y(R+.2,I+.2),Y(R-.2,I+.2)];s.save(),s.globalAlpha=O?1:.6,L(oe,O?p.info:p.rule),s.restore()}function q(){s.save(),s.lineCap="round";for(let z of o){if(z.role==="orchestrator")continue;let[X,Y]=y(0,0),[re,fe]=y(z.cx,z.cy);s.beginPath(),s.moveTo(X,Y),s.lineTo(re,fe),s.globalAlpha=.8,s.strokeStyle=p.rule,s.lineWidth=11*u,s.stroke(),s.globalAlpha=.9,s.strokeStyle=p.sheet,s.lineWidth=6*u,s.stroke()}s.restore();let R=3.5,I=[y(-R,-R),y(R,-R),y(R,R),y(-R,R)];L(I,p.sheet),s.save(),s.strokeStyle=p.rule,s.lineWidth=1,s.beginPath(),I.forEach(([z,X],Y)=>Y===0?s.moveTo(z,X):s.lineTo(z,X)),s.closePath(),s.stroke();let O=2.4;s.globalAlpha=.6,s.setLineDash([4,4]),s.beginPath(),s.moveTo(...y(-O,-O)),s.lineTo(...y(O,-O)),s.lineTo(...y(O,O)),s.lineTo(...y(-O,O)),s.closePath(),s.stroke(),s.restore()}function ee(R,I){let[O,z]=y(R,I,jt);s.save(),s.translate(O,z),s.scale(1,.5);let X=s.createRadialGradient(0,0,0,0,0,36*u);X.addColorStop(0,"rgba(255, 214, 140, 0.5)"),X.addColorStop(1,"rgba(255, 214, 140, 0)"),s.fillStyle=X,s.beginPath(),s.arc(0,0,36*u,0,Math.PI*2),s.fill(),s.restore()}function J(R,I,O,z,X,Y,re,fe){let ge=(oe,xe,_e)=>y(oe,xe,_e),B=z,ue=z+X;L([ge(R+O,I-O,ue),ge(R+O,I+O,ue),ge(R+O,I+O,B),ge(R+O,I-O,B)],fe),L([ge(R+O,I+O,ue),ge(R-O,I+O,ue),ge(R-O,I+O,B),ge(R+O,I+O,B)],re),L([ge(R-O,I-O,ue),ge(R+O,I-O,ue),ge(R+O,I+O,ue),ge(R-O,I+O,ue)],Y)}function ne(R,I,O,z){let X=jt;J(R,I,.42,X,9,"hsl(32 30% 74%)","hsl(32 25% 52%)","hsl(32 25% 60%)");let Y=R,re=I-.12,fe=.12;J(Y,re,fe,X+9,11,p.ink2,p.ink,p.ink2);let ge=(B,ue)=>y(B,re+fe,ue);s.save(),O&&(s.shadowColor=p.info,s.shadowBlur=8+4*Math.sin(z/400)),L([ge(Y-fe*.8,X+11),ge(Y+fe*.8,X+11),ge(Y+fe*.8,X+18),ge(Y-fe*.8,X+18)],O?p.info:p.rule),s.restore()}function ce(R,I){let O=jt;J(R,I,.2,O,6,"hsl(18 35% 52%)","hsl(18 30% 38%)","hsl(18 30% 44%)");let[z,X]=y(R,I,O+6),Y=[[-4,-9,-.6],[4,-9,.6],[0,-13,0],[-2,-6,-1.2],[3,-6,1.1]];s.save(),s.fillStyle="hsl(118 38% 40%)";for(let[re,fe,ge]of Y)s.beginPath(),s.ellipse(z+re*u,X+fe*u,5*u,2.8*u,ge,0,Math.PI*2),s.fill();s.restore()}function pe(R,I){J(R,I,.22,jt,10,"hsl(35 45% 64%)","hsl(35 40% 42%)","hsl(35 40% 50%)")}function me(R,I){let[O,z]=y(R,I,jt),[X,Y]=y(R,I,jt+44);s.save(),s.strokeStyle=p.ink2,s.lineWidth=1.5,s.beginPath(),s.moveTo(O,z),s.lineTo(X,Y),s.stroke(),s.shadowColor="hsl(42 95% 62%)",s.shadowBlur=10,s.fillStyle="hsl(42 95% 74%)",s.beginPath(),s.arc(X,Y,2.8,0,Math.PI*2),s.fill(),s.restore()}function qe(R,I,O){let z=jt;J(R,I,.22,z,26,"hsl(220 10% 34%)","hsl(220 10% 16%)","hsl(220 10% 24%)");let X=I+.22;for(let Y=0;Y<5;Y++)for(let re=0;re<2;re++){let[fe,ge]=y(R-.1+re*.12,X,z+4+Y*4.5),B=Math.sin(O/170+Y*1.7+re*2.9)>.1;s.beginPath(),s.arc(fe,ge,1.3,0,Math.PI*2),s.fillStyle=B?re?p.ok:p.info:p.ink3,s.fill()}}function Le(R,I){let O=jt;J(R,I,.3,O,24,"hsl(28 30% 62%)","hsl(28 28% 40%)","hsl(28 28% 48%)");let z=I+.3;for(let X=0;X<2;X++){let Y=O+4+X*10;for(let re=0;re<6;re++){let fe=R-.26+re*.09,ge=[y(fe,z,Y+7),y(fe+.07,z,Y+7),y(fe+.07,z,Y),y(fe,z,Y)];L(ge,`hsl(${(re*53+X*140)%360} 45% ${X?46:58}%)`)}}}function Je(R,I,O){let z=jt;s.save(),s.strokeStyle=p.ink2,s.lineWidth=1.2;for(let Y of[R-.5,R+.5]){let[re,fe]=y(Y,I,z),[ge,B]=y(Y,I,z+28);s.beginPath(),s.moveTo(re,fe),s.lineTo(ge,B),s.stroke()}let X=[y(R-.55,I,z+26),y(R+.55,I,z+26),y(R+.55,I,z+12),y(R-.55,I,z+12)];L(X,p.sheet),s.strokeStyle=p.ink3,s.lineWidth=1,s.beginPath(),X.forEach(([Y,re],fe)=>fe===0?s.moveTo(Y,re):s.lineTo(Y,re)),s.closePath(),s.stroke(),s.strokeStyle=O?p.info:p.signal,s.beginPath();for(let Y=0;Y<3;Y++){let re=z+22-Y*3.5,[fe,ge]=y(R-.4,I,re),[B,ue]=y(R-.4+.25+Y*.12,I,re);s.moveTo(fe,ge),s.lineTo(B,ue)}s.stroke(),s.restore()}function se(R,I){J(R,I,.3,0,6,"hsl(30 22% 66%)","hsl(30 20% 44%)","hsl(30 20% 52%)")}function le(R,I){let O=a.some(z=>z.district===R.d&&z.agent.status==="working");switch(R.kind){case"desk":ne(R.x,R.y,O,I);break;case"plant":ce(R.x,R.y);break;case"crate":pe(R.x,R.y);break;case"lamp":me(R.x,R.y);break;case"rack":qe(R.x,R.y,I);break;case"shelf":Le(R.x,R.y);break;case"board":Je(R.x,R.y,O);break;case"bench":se(R.x,R.y);break}}function Se(R){let I=o.find(re=>re.role==="orchestrator");if(!I)return;let[O,z]=y(I.cx,I.cy,jt+60+Math.sin(R/900)*2),X=22*u,Y=R/2600;s.save(),s.globalAlpha=.7,s.strokeStyle=p.signal,s.lineWidth=1.4,s.setLineDash([6*u,5*u]),s.lineDashOffset=-R/60,s.beginPath(),s.ellipse(O,z,X,X*.36,Y,0,Math.PI*2),s.stroke(),s.globalAlpha=.4,s.strokeStyle=p.info,s.setLineDash([]),s.beginPath(),s.ellipse(O,z,X*.66,X*.24,-Y*1.6,0,Math.PI*2),s.stroke(),s.restore()}function ke(R,I){let[O,z]=y(R.x,R.y,jt),X=R.agent.status,Y=X==="working"?p.info:X==="idle"?p.ok:p.ink3,re=R.district.hue,fe=R.moving?Math.sin(R.walk)*3:0,ge=ue=>X==="working"?Math.sin(I/110+(ue>0?Math.PI:0))*2:0;s.save(),s.globalAlpha=X==="offline"?.4:1,s.translate(O,z),s.scale(u*R.facing,u),s.save(),s.globalAlpha*=.18,s.fillStyle=p.ink,s.beginPath(),s.ellipse(0,0,9,4.5,0,0,Math.PI*2),s.fill(),s.restore(),s.strokeStyle=p.ink2,s.lineWidth=2.5,s.lineCap="round";for(let ue of[-1,1]){let oe=ue===-1?fe:-fe;s.beginPath(),s.moveTo(ue*3.5,-12),s.lineTo(ue*3.5+oe,0),s.stroke()}let B=s.createLinearGradient(0,-30,0,-12);B.addColorStop(0,`hsl(${re} 55% 66%)`),B.addColorStop(1,`hsl(${re} 50% 44%)`),U(-7,-30,14,18,4),s.fillStyle=B,s.fill(),s.strokeStyle=p.ink2,s.lineWidth=.8,s.stroke(),U(-4,-25,8,7,1.5),s.fillStyle=p.sheet,s.fill(),s.fillStyle=p.ink,s.font="bold 7px ui-monospace, SFMono-Regular, Menlo, monospace",s.textAlign="center",s.textBaseline="middle",s.fillText(R.agent.role.charAt(0).toUpperCase()||"?",0,-21.5),s.strokeStyle=`hsl(${re} 45% 36%)`,s.lineWidth=2;for(let ue of[-1,1])s.beginPath(),s.moveTo(ue*7,-27),s.lineTo(ue*10,-19+ge(ue)),s.stroke();U(-6,-41,12,10,3),s.fillStyle=p.sheet,s.fill(),s.strokeStyle=p.ink2,s.stroke(),U(-4.5,-38.5,9,5,1.5),s.fillStyle=p.ink,s.fill(),s.fillStyle=Y;for(let ue of[-2,2])s.beginPath(),s.arc(ue,-36,1.1,0,Math.PI*2),s.fill();if(s.strokeStyle=p.ink2,s.lineWidth=.8,s.beginPath(),s.moveTo(0,-41),s.lineTo(0,-46),s.stroke(),s.beginPath(),s.arc(0,-47,1.8,0,Math.PI*2),s.fillStyle=Y,s.fill(),X==="working"){let ue=.5+.5*Math.sin(I/300);s.shadowColor=p.info,s.shadowBlur=8+6*ue,s.beginPath(),s.arc(0,-56,2.6+ue*.6,0,Math.PI*2),s.fillStyle=p.info,s.fill(),s.shadowBlur=0}s.restore()}function ye(R,I){let[O,z]=y(R.x,R.y,jt);if(R.bubble&&I<R.bubble.until){let[X,Y]=y(R.x,R.y,jt+52),re=br((R.bubble.until-I)/800,0,1),fe=Kt(R.bubble.text,30);s.font="11px ui-sans-serif, system-ui, sans-serif";let ge=s.measureText(fe).width+14,B=18;s.save(),s.globalAlpha=re,U(X-ge/2,Y-B-6,ge,B,6),s.fillStyle=p.sheet,s.fill(),s.strokeStyle=R.bubble.tone==="danger"?p.danger:R.bubble.tone==="ok"?p.ok:p.info,s.lineWidth=1,s.stroke(),s.fillStyle=p.ink,s.textAlign="center",s.textBaseline="middle",s.fillText(fe,X,Y-B/2-6),s.restore()}(R===f||R===d||u>.9)&&(s.font="600 11px ui-sans-serif, system-ui, sans-serif",s.textAlign="center",s.textBaseline="top",s.lineJoin="round",s.lineWidth=3,s.strokeStyle=p.paper,s.strokeText(R.agent.name,O,z+14),s.fillStyle=p.ink,s.fillText(R.agent.name,O,z+14))}function je(R){for(let I=h.length-1;I>=0;I--){let O=h[I];if(!O)continue;let z=(R-O.start)/A_;if(z>=1){h.splice(I,1);continue}let X=l.get(O.from),Y=l.get(O.to);if(!X||!Y)continue;let[re,fe]=y(X.x,X.y,jt+df),[ge,B]=y(Y.x,Y.y,jt+df),ue=(re+ge)/2,oe=(fe+B)/2-60*u,xe=1-z,_e=xe*xe*re+2*xe*z*ue+z*z*ge,he=xe*xe*fe+2*xe*z*oe+z*z*B,we=O.tone==="ok"?p.ok:p.signal;s.save(),s.shadowColor=we,s.shadowBlur=12,s.fillStyle=we,s.beginPath(),s.arc(_e,he,3.5,0,Math.PI*2),s.fill(),s.restore()}}function Nt(R){s.setTransform(x,0,0,x,0,0),s.globalAlpha=1,s.fillStyle=p.paper,s.fillRect(0,0,g,_);let I=8;for(let z of o)I=Math.max(I,Math.abs(z.cx)+z.r+3,Math.abs(z.cy)+z.r+3);s.save(),s.globalAlpha=.3,s.strokeStyle=p.rule,s.lineWidth=1,s.beginPath();for(let z=-I;z<=I;z++){let[X,Y]=y(z,-I),[re,fe]=y(z,I);s.moveTo(X,Y),s.lineTo(re,fe);let[ge,B]=y(-I,z),[ue,oe]=y(I,z);s.moveTo(ge,B),s.lineTo(ue,oe)}s.stroke(),s.restore(),q();for(let z of o)F(z);for(let z of c)z.kind==="lamp"&&ee(z.x,z.y);let O=[];for(let z of c)O.push({depth:z.x+z.y,paint:()=>le(z,R)});for(let z of a){let X=z.agent.status==="working",[Y,re]=z.home;O.push({depth:Y+re-.01,paint:()=>H(Y,re,X)}),O.push({depth:z.x+z.y,paint:()=>ke(z,R)})}O.sort((z,X)=>z.depth-X.depth);for(let z of O)z.paint();Se(R),je(R);for(let z of a)ye(z,R);Qt()}function Ye(R,I,O){R!==I&&(h.push({from:R,to:I,start:performance.now(),tone:O}),h.length>C_&&h.shift())}function nt(R,I,O){R.bubble={text:I,until:performance.now()+T_,tone:O}}function ft(R,I){let O=null,z=22*u;for(let X of a){let[Y,re]=y(X.x,X.y,jt),fe=Math.hypot(R-Y,I-(re-24*u));fe<z&&(z=fe,O=X)}return O}function Qe(R){d=R,S="",W()}let Tt=R=>R.agent.status==="offline"?"offline":R.activity,Bt=R=>{let I=typeof R=="number"?l.get(R):void 0;return I?I.agent.name:String(R??"?")};function $t(R,I){R.activity=I,R===d&&W()}function wt(R,I){let O=I.to_cid??I.assignee;return(R==="plaza_send_message"||(R==="plaza_assign"||R==="plaza_post_task"))&&typeof O=="number"?O:null}function Ut(R,I){if(R==="bash")return`$ ${Kt(Ii(I.command).split(`
`)[0]??"",120)}`;if(R==="plaza_send_message")return`\u2709 to ${Bt(I.to_cid)}: ${Kt(Ii(I.text),100)}`;if(R==="plaza_assign"||R==="plaza_post_task"&&I.assignee!=null)return`\u2192 task for ${Bt(I.assignee??I.to_cid)}: ${Ii(I.title)}`;if(R==="share_file")return`\u21EA shared ${Ii(I.path)}`;let O=Ii(I.path)||Ii(I.url);return O?`\u2192 ${R} ${Kt(O,80)}`:`\u2192 ${R}`}function W(){if(!d){t.hidden=!0,S="";return}let R=d.agent;t.hidden=!1;let I=Tt(d),O=[R.name,I].join("|");O!==S&&(S=O,t.replaceChildren(v("header.floor-card-head",null,v("strong",null,R.name)),v("div.floor-card-activity",null,I)))}function Qt(){if(!d||t.hidden)return;let[R,I]=y(d.x,d.y,jt+40),O=br(R+24,8,Math.max(8,g-308)),z=br(I-120,8,Math.max(8,_-260));t.style.left=`${O}px`,t.style.top=`${z}px`}let at=R=>{let I=e.getBoundingClientRect();return[R.clientX-I.left,R.clientY-I.top]};e.addEventListener("pointerdown",R=>{P={x:R.clientX,y:R.clientY,moved:0},e.setPointerCapture(R.pointerId),e.style.cursor="grabbing"}),e.addEventListener("pointermove",R=>{if(P){let z=R.clientX-P.x,X=R.clientY-P.y;P.moved+=Math.abs(z)+Math.abs(X),P.x=R.clientX,P.y=R.clientY,m={x:m.x+z,y:m.y+X};return}let[I,O]=at(R);f=ft(I,O),e.style.cursor=f?"pointer":"grab"}),e.addEventListener("pointerup",R=>{if(!P)return;let I=P.moved;if(P=null,e.style.cursor="grab",I<4){let[O,z]=at(R);Qe(ft(O,z))}}),e.addEventListener("pointercancel",()=>{P=null,e.style.cursor="grab"}),e.addEventListener("pointerleave",()=>{f=null}),e.addEventListener("wheel",R=>{R.preventDefault();let[I,O]=at(R),z=g/2+m.x,X=_/2+m.y,Y=(I-z)/u,re=(O-X)/u;u=br(u*Math.exp(-R.deltaY*.0015),.4,2.5),m={x:I-Y*u-g/2,y:O-re*u-_/2}},{passive:!1});let N=()=>{x=window.devicePixelRatio||1,g=e.clientWidth,_=e.clientHeight,e.width=Math.round(g*x),e.height=Math.round(_*x)},b=new ResizeObserver(()=>N());b.observe(e),N();let Z=ut.watch(R=>{M(R),!w&&R.length&&(C(),w=!0)}),te=bt("transcript",R=>{let I=R,O=l.get(I.cid);switch(I.kind){case"tool_request":{let z=Ii(I.data.tool)||Ii(I.data.name)||"tool",X=I.data.input??{};O&&(nt(O,`\u2192 ${z}`,"info"),$t(O,Ut(z,X)));let Y=wt(z,X);Y!==null&&Ye(I.cid,Y,"accent");break}case"model_request":O&&$t(O,"thinking\u2026");break;case"work_end":{let z=Ii(I.data.status)||"done";O&&(nt(O,z,/err|fail/i.test(z)?"danger":"ok"),$t(O,"idle"));break}case"work_start":{let z=I.data.origin??{};if(typeof z.from_cid=="number")Ye(z.from_cid,I.cid,"accent");else if(I.data.kind==="task"){let X=a.find(Y=>Y.agent.tier==="orchestrator");X&&Ye(X.agent.cid,I.cid,"accent")}break}default:break}}),ae=bt("delta",R=>{let I=l.get(R.cid);I&&$t(I,"thinking\u2026")});e.addEventListener("dblclick",R=>{let[I,O]=at(R),z=ft(I,O);z&&_n("agents",z.agent.cid)});let ve=bt("conversation",R=>{let I=R;Ye(I.from_cid,I.to_cid,"ok")}),Me=R=>{if(!T)return;let I=Math.min(.05,(R-D)/1e3);D=R,k(I,R),Nt(R),A=requestAnimationFrame(Me)};return A=requestAnimationFrame(Me),()=>{T=!1,cancelAnimationFrame(A),b.disconnect(),Z(),te(),ae(),ve(),n.classList.remove("floor-view"),n.replaceChildren()}}var z_=1500,H_=1.6,V_=60,vo=.5,Ll=.8,Il=2.3,kl=1.5,gf=2.2,G_=9.8,W_=.95,X_=Math.PI/4,q_=9.5,xf=3.1,_o=.45,$_=[0,30,-30,60,-60].map(n=>n*Math.PI/180),Eu=90,Y_="position:absolute;left:0;top:0;display:none;max-width:220px;padding:3px 6px;background:var(--sheet);border:1px solid var(--ink);color:var(--ink);font:11px/1.35 ui-monospace,SFMono-Regular,Menlo,monospace;white-space:nowrap;pointer-events:none;will-change:transform";function Z_(n){let e=0;for(let t of n)e=e*31+t.charCodeAt(0)>>>0;return e%360}function K_(n){let e=new Map;for(let h of n){let m=h.tier==="orchestrator"?"":h.role;e.set(m,[...e.get(m)??[],h])}let t=[],i=new Map,r=(h,m)=>{t.push(h);let u=Math.max(1,Math.ceil(Math.sqrt(m.length)));m.sort((g,_)=>g.cid-_.cid).forEach((g,_)=>{let x=1.5*(u-1),f=h.cx-x+_%u*3,d=h.cy-x+Math.floor(_/u)*3;i.set(g.cid,[h,f,d])})},s=e.get("");s&&r({role:"orchestrator",cx:0,cy:0,r:2,hue:25},s);let o=[...e.keys()].filter(h=>h).sort(),a=h=>Math.ceil(1.5*Math.max(1,Math.ceil(Math.sqrt(h)))+1),c=o.reduce((h,m)=>Math.max(h,a(e.get(m).length)),0),l=Math.max(9+kl,Math.ceil((2*(c+kl)+1.5)/(2*Math.sin(Math.PI/Math.max(3,o.length)))));return o.forEach((h,m)=>{let u=e.get(h),g=m/o.length*Math.PI*2-Math.PI/2;r({role:h,cx:Math.round(Math.cos(g)*l),cy:Math.round(Math.sin(g)*l),r:a(u.length),hue:Z_(h)},u)}),{districts:t,homes:i}}function J_(){let n=getComputedStyle(document.documentElement),e=(t,i)=>n.getPropertyValue(`--${t}`).trim()||i;return{paper:e("paper","#f5f3ee"),sheet:e("sheet","#ffffff"),rule:e("rule","#d6d2c8"),ink:e("ink","#1a1a1a"),ink2:e("ink-2","#4a4a4a"),ink3:e("ink-3","#8a8a8a"),signal:e("signal","#d9482b"),ok:e("ok","#2f8f5b"),info:e("info","#2f6fb0"),danger:e("danger","#c0392b")}}var Mr=null;function Sr(n,e){Mr??=document.createElement("canvas").getContext("2d");let t=new ze;if(!Mr)return t.set(e);Mr.fillStyle=e,Mr.fillStyle=n,Mr.clearRect(0,0,1,1),Mr.fillRect(0,0,1,1);let i=Mr.getImageData(0,0,1,1).data;return t.setRGB((i[0]??0)/255,(i[1]??0)/255,(i[2]??0)/255,Ht)}function er(n){return typeof n=="string"?n:""}function vf(n,e,t){return Math.min(t,Math.max(e,n))}var xo=n=>n.role==="orchestrator"?Ll:vo,_f=(n,e,t)=>e>n.x0-_o&&e<n.x1+_o&&t>n.z0-_o&&t<n.z1+_o;function wr(n){n.traverse(e=>{let t=e;t.geometry?.dispose();let i=Array.isArray(t.material)?t.material:t.material?[t.material]:[];for(let r of i)r.map?.dispose(),r.dispose()})}function j_(n,e){let t=e.to_cid??e.assignee;return(n==="plaza_send_message"||(n==="plaza_assign"||n==="plaza_post_task"))&&typeof t=="number"?t:null}function Q_(n,e,t){if(n==="bash")return`$ ${Kt(er(e.command).split(`
`)[0]??"",120)}`;if(n==="plaza_send_message")return`\u2709 to ${t(e.to_cid)}: ${Kt(er(e.text),100)}`;if(n==="plaza_assign"||n==="plaza_post_task"&&e.assignee!=null)return`\u2192 task for ${t(e.assignee??e.to_cid)}: ${er(e.title)}`;if(n==="share_file")return`\u21EA shared ${er(e.path)}`;let i=er(e.path)||er(e.url);return i?`\u2192 ${n} ${Kt(i,80)}`:`\u2192 ${n}`}function ey(){try{return new Sl({antialias:!0})}catch{return null}}function yf(n){let e=document.createElement("canvas"),i=e.getContext("webgl2")??e.getContext("webgl")?ey():null;if(!i)return n.replaceChildren(v("p.faint",null,"3D unavailable (no WebGL) \u2014 showing the 2D floor.")),mf(n);let r=J_(),s={sheet:Sr(r.sheet,"#ffffff"),rule:Sr(r.rule,"#d6d2c8"),ink3:Sr(r.ink3,"#8a8a8a"),signal:Sr(r.signal,"#d9482b"),ok:Sr(r.ok,"#2f8f5b"),info:Sr(r.info,"#2f6fb0"),paper:Sr(r.paper,"#f5f3ee")},o={ink:r.ink,sheet:r.sheet,paper:r.paper,rule:r.rule,signal:r.signal,ok:r.ok,info:r.info,inkMuted:r.ink3};i.setPixelRatio(Math.min(window.devicePixelRatio||1,2)),i.shadowMap.enabled=!0,i.shadowMap.type=Pa;let a=i.domElement;a.classList.add("floor-canvas"),a.setAttribute("aria-label","The plaza floor"),a.style.cssText="position:absolute;inset:0;width:100%;height:100%;display:block;cursor:grab";let c=new Us;c.background=s.paper;let l=new mn(40,1,.5,1e3),h=new Rl(l,a);h.enableDamping=!0,h.dampingFactor=.08,h.enablePan=!0,h.enableZoom=!0,h.minPolarAngle=.2,h.maxPolarAngle=1.3,h.minDistance=6,h.maxDistance=300,c.add(new qs(16774114,11772298,1.6));let m=new Ks(16777215,1.4);m.position.set(22,40,16),m.castShadow=!0,m.shadow.mapSize.set(2048,2048),m.shadow.bias=-8e-4;let u=m.shadow.camera;Object.assign(u,{left:-40,right:40,top:40,bottom:-40,near:1,far:140}),u.updateProjectionMatrix(),c.add(m);let g=new xt(new Gi(200,200),new St({map:Qd(),roughness:1}));g.rotation.x=-Math.PI/2,g.position.y=-.01,g.receiveShadow=!0,c.add(g);let _=new js(200,100,s.rule,s.rule),x=_.material;x.transparent=!0,x.opacity=.35,x.depthWrite=!1,c.add(_);let f=new _t;c.add(f);let d=new _t;c.add(d);let S=lf(s);c.add(S);let P=S.userData.animate,w=new pi(.42,28),T=new Jt(.13,14,10),A=new Jt(.34,14,10),D=new Ke(.2,.2,.2),p={accent:new on({color:s.signal,toneMapped:!1}),ok:new on({color:s.ok,toneMapped:!1}),accentHalo:new on({color:s.signal,transparent:!0,opacity:.35,depthWrite:!1,blending:ls,toneMapped:!1}),okHalo:new on({color:s.ok,transparent:!0,opacity:.35,depthWrite:!1,blending:ls,toneMapped:!1}),token:new on({color:s.signal,toneMapped:!1})},M=new Float32Array(Eu*3),y=new Pn(M,3),C=[];for(let $=0;$<Eu;$++){let de=Math.random()*Math.PI*2,be=Math.sqrt(Math.random())*9;C.push({x:Math.cos(de)*be,y:Math.random()*4.2,z:Math.sin(de)*be,rise:.04+Math.random()*.05})}let k=new Xt;k.setAttribute("position",y);let L=new ts({color:s.ink3,size:.07,transparent:!0,opacity:.55,depthWrite:!1});c.add(new Bs(k,L));let U=v("div.floor-card",{hidden:!0}),F=v("div.floor-legend",null,v("span",null,v("i.floor-key.working"),"Working"),v("span",null,v("i.floor-key.idle"),"Idle"),v("span",null,v("i.floor-key.offline"),"Offline")),H=document.createElement("div");H.style.cssText="position:absolute;inset:0;overflow:hidden;pointer-events:none";let q=v("div.floor-stage",null,H,U,F);q.style.position="relative",q.prepend(a),n.classList.add("floor-view"),document.body.classList.add("floor-mode"),n.append(v("div.view-head",null,v("h1",null,"Floor"),v("div.actions",null,We("Reset view",()=>W(),{kind:"ghost",icon:"refresh"}))),q);let ee=[],J="",ne=[],ce=new Map,pe=[],me=[],qe=new Map,Le=[],Je=new V,se=0,le=0,Se=null,ke=null,ye="",je=!1,Nt=performance.now(),Ye=new He,nt=new Js,ft=new V;function Qe($,de){let be=document.createElement("canvas");be.width=512,be.height=128;let K=be.getContext("2d");K.beginPath(),K.moveTo(24,4),K.arcTo(508,4,508,124,20),K.arcTo(508,124,4,124,20),K.arcTo(4,124,4,4,20),K.arcTo(4,4,508,4,20),K.closePath(),K.fillStyle=r.sheet,K.fill(),K.lineWidth=6,K.strokeStyle=de?r.signal:r.rule,K.stroke(),K.fillStyle=r.ink2,K.textAlign="center",K.textBaseline="middle";let Pe="500 48px ui-monospace, SFMono-Regular, Menlo, monospace";K.font=Pe;let Be=K.measureText($).width;Be>460&&(K.font=`500 ${Math.floor(48*460/Be)}px ui-monospace, SFMono-Regular, Menlo, monospace`),K.fillText($,256,66);let pt=new dr(be);return pt.colorSpace=Ht,pt}function Tt($){let de=new _t,be=$.role==="orchestrator";if(be){let Pe=new xt(new Dt(Il,Il+gf,Ll,48),new St({map:Cl(s.sheet),roughness:.35,metalness:.05}));Pe.position.set($.cx,Ll/2,$.cy),Pe.castShadow=!0,Pe.receiveShadow=!0;let Be=new xt(new fr(Il,.05,8,64),new on({color:s.signal}));Be.rotation.x=Math.PI/2,Be.position.set($.cx,Ll+.01,$.cy),de.add(Pe,Be)}else{let Pe=new Dt($.r*Math.SQRT2,($.r+kl)*Math.SQRT2,vo,4,1);Pe.rotateY(Math.PI/4);let Be=new xt(Pe,[Mu($.hue),new St({map:ef($.hue),roughness:.95}),Mu($.hue)]);Be.position.set($.cx,vo/2,$.cy),Be.castShadow=!0,Be.receiveShadow=!0;let pt=new es(new Gs(Pe),new hr({color:s.rule}));pt.position.copy(Be.position),de.add(Be,pt);let $e=af($.hue,$.r,$.role,s);$e.position.set($.cx,vo,$.cy),de.add($e),$e.updateMatrixWorld(!0);for(let Ct of $e.children){let sn=new Xn().setFromObject(Ct);sn.min.y>vo+2.2||me.push({x0:sn.min.x,x1:sn.max.x,z0:sn.min.z,z1:sn.max.z})}qe.set($.role,{screens:$e.userData.screens??[],lit:!1});let en=$e.userData.animate;en&&Le.push(en)}let K=new xt(new Gi(2.6,.65),new on({map:Qe($.role.toUpperCase(),be),transparent:!0,depthWrite:!1}));return K.rotation.x=-Math.PI/2,K.position.set($.cx,xo($)+.01,be?$.cy-1.6:$.cy-$.r+.55),de.add(K),de}function Bt($){for(let K of[...f.children])f.remove(K),wr(K);qe.clear(),Le.length=0,me.length=0;for(let K of $)f.add(Tt(K));wr(d),d.clear(),d.add(uf($.map(K=>new V(K.cx,0,K.cy)),s));let de=cf(s,$.map(K=>Math.atan2(K.cy,K.cx)));d.add(de.group),Le.push(de.animate);for(let K of de.boxes)me.push(K);let be=hf($,s);d.add(be.group);for(let K of be.boxes)me.push(K)}function $t($,de,be,K){let Pe=$.tier==="orchestrator",Be=Kd(de.hue,Pe,o);Be.group.traverse(Li=>{Li.castShadow=!0,Li.receiveShadow=!0}),c.add(Be.group);let pt=new on({color:s.ink3,transparent:!0,opacity:.6,depthWrite:!1}),$e=new xt(w,pt);$e.rotation.x=-Math.PI/2,$e.position.set(be,xo(de)+.02,K),c.add($e);let en=new xt(D,p.token);en.visible=!1,c.add(en);let Ct=document.createElement("div");Ct.style.cssText=Y_;let sn=document.createElement("strong");sn.style.cssText="display:block;overflow:hidden;text-overflow:ellipsis";let Hn=document.createElement("div");return Hn.style.cssText="color:var(--ink-2);overflow:hidden;text-overflow:ellipsis",Ct.append(sn,Hn),H.append(Ct),{agent:$,district:de,home:[be,K],x:be,y:K,tx:be,ty:K,heading:0,moving:!1,py:0,vy:0,state:null,nextWander:performance.now()+Math.random()*3e3,nextSocial:performance.now()+4e3+Math.random()*1e4,social:null,activity:"idle",robot:Be,robotKey:`${de.hue}|${Pe}`,pad:$e,padMat:pt,token:en,highlighted:!1,label:Ct,labelName:sn,labelLine:Hn}}function wt($){c.remove($.robot.group),$.robot.dispose(),c.remove($.pad),$.padMat.dispose(),c.remove($.token),$.label.remove()}function Ut($){let de=K_($);ee=de.districts;let be=JSON.stringify(ee);be!==J&&(J=be,Bt(ee));let K=new Set;for(let Pe of $){let Be=de.homes.get(Pe.cid);if(!Be)continue;let[pt,$e,en]=Be;K.add(Pe.cid);let Ct=ce.get(Pe.cid);Ct&&Ct.robotKey!==`${pt.hue}|${Pe.tier==="orchestrator"}`&&(wt(Ct),Ct=void 0),Ct||(Ct=$t(Pe,pt,$e,en),ce.set(Pe.cid,Ct)),Object.assign(Ct,{agent:Pe,district:pt,home:[$e,en]}),Ct.pad.position.set($e,xo(pt)+.02,en)}for(let[Pe,Be]of ce)K.has(Pe)||(wt(Be),ce.delete(Pe));ne=[...ce.values()],ke&&(ke=ce.get(ke.agent.cid)??null),Se&&(Se=ce.get(Se.agent.cid)??null),re()}function W(){let $=6;for(let be of ee)$=Math.max($,Math.abs(be.cx)+be.r,Math.abs(be.cy)+be.r);let de=$*2.6+6;h.target.set(0,.4,0),l.position.copy(h.target).add(new V().setFromSphericalCoords(de,W_,X_)),h.update()}let Qt=($,de)=>$.role===de.role||Math.hypot($.cx-de.cx,$.cy-de.cy)<=q_;function at($,de){if($.nextSocial=de+8e3+Math.random()*12e3,Math.random()<.3){let Pe=Math.atan2($.y,$.x);$.social={phase:"stroll",partner:null,gx:Math.cos(Pe)*xf,gy:Math.sin(Pe)*xf,until:1/0};return}let be=ne.filter(Pe=>Pe!==$&&Pe.agent.status==="idle"&&!Pe.social&&Qt(Pe.district,$.district)),K=be[Math.floor(Math.random()*be.length)];K&&($.social={phase:"go",partner:K,gx:K.x-1,gy:K.y,until:1/0},K.social={phase:"talk",partner:$,gx:K.x,gy:K.y,until:1/0})}function N($,de){let be=$.social;$.social=null,$.nextWander=de,be?.partner&&be.partner.social?.partner===$&&(be.partner.social=null,be.partner.nextWander=de)}function b($,de){let be=0;for(let K of ee){let Pe=K.role==="orchestrator",Be=Pe?Math.hypot($-K.cx,de-K.cy)-Il:Math.max(Math.abs($-K.cx),Math.abs(de-K.cy))-K.r,pt=Pe?gf:kl,$e=Be<=0?xo(K):Be<pt?xo(K)*(1-Be/pt):0;be=Math.max(be,$e)}return be}function Z($,de,be,K){if(K){for(let Be of me)if(_f(Be,de,be)&&!_f(Be,$.x,$.y))return!0}let Pe=$.social?.partner??null;for(let Be of ne){if(Be===$||Be===Pe)continue;let pt=Math.hypot(Be.x-de,Be.y-be);if(pt<_o*2&&pt<Math.hypot(Be.x-$.x,Be.y-$.y))return!0}return!1}function te($,de){let be=de/1e3;for(let K of qe.values())K.lit=!1;for(let K of ne){let Pe=K.agent.status,Be=Pe==="idle",pt=K.social?.partner??null;if(K.social&&(!Be||pt&&(ce.get(pt.agent.cid)!==pt||pt.agent.status!=="idle"))&&N(K,de),Be&&!K.social){if(de>=K.nextWander){let E=Math.max(.5,K.district.r-.7);K.tx=K.district.cx+(Math.random()*2-1)*E,K.ty=K.district.cy+(Math.random()*2-1)*E,K.nextWander=de+3e3+Math.random()*4e3}de>=K.nextSocial&&at(K,de)}let $e=K.social,en=K.home[0],Ct=K.home[1];Be&&(en=$e?$e.gx:K.tx,Ct=$e?$e.gy:K.ty);let sn=en-K.x,Hn=Ct-K.y,Li=Math.hypot(sn,Hn);K.moving=Li>.02;let Er=Math.hypot(K.home[0]-K.x,K.home[1]-K.y)>.9,xs=null;if(K.moving){let E=Math.min(H_*$,Li),G=Math.atan2(sn,Hn);for(let ie of $_){let j=G+ie,Q=K.x+Math.sin(j)*E,Ee=K.y+Math.cos(j)*E;if(!Z(K,Q,Ee,Er)){K.x=Q,K.y=Ee,xs=j;break}}}if($e&&Be){if($e.phase==="go"&&!K.moving){let E=de+3e3+Math.random()*2e3;$e.phase="talk",$e.until=E,$e.partner?.social&&($e.partner.social.until=E)}else $e.phase==="stroll"&&!K.moving&&$e.until===1/0&&($e.until=de+2e3);de>=$e.until&&N(K,de)}let Tr=$e&&$e.phase==="talk"?$e.partner:null,tr=null;if(Tr?tr=Math.atan2(Tr.x-K.x,Tr.y-K.y):K.moving&&(tr=xs??Math.atan2(sn,Hn)),tr!==null){let E=Math.atan2(Math.sin(tr-K.heading),Math.cos(tr-K.heading));K.heading+=E*Math.min(1,$*8)}let vs=Pe==="working"?"working":Pe==="offline"?"offline":K.moving?"walking":"idle";vs!==K.state&&(K.state=vs,K.robot.setState(vs));let _s=K===ke||K===Se;_s!==K.highlighted&&(K.highlighted=_s,K.robot.setHighlighted(_s));let Ar=b(K.x,K.y);K.py>Ar+.001?(K.vy-=G_*$,K.py=Math.max(Ar,K.py+K.vy*$),K.py===Ar&&(K.vy=0)):(K.py=Ar,K.vy=0),K.robot.group.position.set(K.x,K.py,K.y),K.robot.group.rotation.y=K.heading,K.robot.update($,be),K.padMat.color.copy(Pe==="working"?s.info:Pe==="idle"?s.ok:s.ink3);let ys=Pe==="working";if(K.token.visible=ys,ys){let E=qe.get(K.district.role);E&&(E.lit=!0);let G=K.robot.labelAnchor(Je);K.token.position.set(G.x,G.y+.6+Math.sin(be*2.5+K.agent.cid)*.08,G.z),K.token.rotation.y+=$*1.8}}for(let K of qe.values()){let Pe=K.lit?1.2:.05;for(let Be of K.screens)Be.emissiveIntensity!==Pe&&(Be.emissiveIntensity=Pe)}}function ae($){for(let de=0;de<Eu;de++){let be=C[de],K=de*3;M[K]=be.x+Math.sin($*.3+de)*.25,M[K+1]=.3+(be.y+$*be.rise)%4.2,M[K+2]=be.z+Math.cos($*.25+de)*.25}y.needsUpdate=!0}function ve($,de,be){if($===de)return;let K=new xt(T,be==="ok"?p.ok:p.accent),Pe=new xt(A,be==="ok"?p.okHalo:p.accentHalo);for(c.add(K,Pe),pe.push({from:$,to:de,start:performance.now(),core:K,halo:Pe});pe.length>V_;)Me(pe.shift())}function Me($){c.remove($.core,$.halo)}function R($){for(let de=pe.length-1;de>=0;de--){let be=pe[de],K=($-be.start)/z_,Pe=ce.get(be.from),Be=ce.get(be.to);if(K>=1||!Pe||!Be){Me(be),pe.splice(de,1);continue}let pt=Pe.robot.labelAnchor(new V),$e=Be.robot.labelAnchor(new V),en=pt.clone().lerp($e,.5);en.y+=.8+pt.distanceTo($e)*.3;let Ct=1-K,sn=pt.multiplyScalar(Ct*Ct).addScaledVector(en,2*Ct*K).addScaledVector($e,K*K);be.core.position.copy(sn),be.halo.position.copy(sn)}}let I=$=>$.agent.status==="offline"?"offline":$.social?.phase==="talk"?"\xB7 \xB7 \xB7":$.activity,O=$=>{let de=typeof $=="number"?ce.get($):void 0;return de?de.agent.name:String($??"?")};function z($,de){$.activity=de,$===ke&&re()}function X($){let de=$.clone().project(l);l.getWorldDirection(ft);let be=$.clone().sub(l.position).dot(ft)>0;return{x:(de.x+1)/2*se,y:(1-de.y)/2*le,visible:be&&de.z<=1}}function Y(){for(let $ of ne){let de=X($.robot.labelAnchor(new V));if(!de.visible){$.label.style.display="none";continue}$.label.style.display="block",$.label.style.transform=`translate(${de.x.toFixed(1)}px, ${de.y.toFixed(1)}px) translate(-50%, -100%) translateY(-6px)`,$.labelName.textContent!==$.agent.name&&($.labelName.textContent=$.agent.name);let be=I($);$.labelLine.textContent!==be&&($.labelLine.textContent=be)}}function re(){if(!ke){U.hidden=!0,ye="";return}let $=ke.agent;U.hidden=!1;let de=I(ke),be=[$.name,de].join("|");be!==ye&&(ye=be,U.replaceChildren(v("header.floor-card-head",null,v("strong",null,$.name)),v("div.floor-card-activity",null,de)))}function fe(){if(!ke||U.hidden)return;let $=X(ke.robot.labelAnchor(new V)),de=vf($.x+24,8,Math.max(8,se-308)),be=vf($.y-120,8,Math.max(8,le-260));U.style.left=`${de}px`,U.style.top=`${be}px`}function ge($){ke=$,ye="",re()}function B($){let de=a.getBoundingClientRect();Ye.set(($.clientX-de.left)/de.width*2-1,-(($.clientY-de.top)/de.height)*2+1),nt.setFromCamera(Ye,l);let be=nt.intersectObjects(ne.map(K=>K.robot.group),!0);for(let K of be){let Pe=K.object;for(;Pe;){let Be=ne.find(pt=>pt.robot.group===Pe);if(Be)return Be;Pe=Pe.parent}}return null}function ue($,de){return Q_($,de,O)}let oe=()=>{let $=q.getBoundingClientRect();se=$.width,le=$.height,!(!se||!le)&&(i.setSize(se,le,!1),l.aspect=se/le,l.updateProjectionMatrix())},xe=new ResizeObserver(()=>oe());xe.observe(q),oe(),W();let _e=new AbortController,he=($,de)=>a.addEventListener($,de,{signal:_e.signal}),we=null;he("pointerdown",$=>{we={x:$.clientX,y:$.clientY}}),he("pointermove",$=>{$.buttons===0&&(Se=B($),a.style.cursor=Se?"pointer":"grab")}),he("pointerup",$=>{if(!we)return;let de=Math.hypot($.clientX-we.x,$.clientY-we.y);we=null,de<4&&ge(B($))}),he("pointerleave",()=>{Se=null}),he("dblclick",$=>{let de=B($);de&&_n("agents",de.agent.cid)});let Re=ut.watch($=>{Ut($),!je&&$.length&&(W(),je=!0)}),vt=bt("transcript",$=>{let de=$,be=ce.get(de.cid);switch(de.kind){case"tool_request":{let K=er(de.data.tool)||er(de.data.name)||"tool",Pe=de.data.input??{};be&&z(be,ue(K,Pe));let Be=j_(K,Pe);Be!==null&&ve(de.cid,Be,"accent");break}case"model_request":be&&z(be,"thinking\u2026");break;case"work_end":be&&z(be,"idle");break;case"work_start":{let K=de.data.origin??{};if(typeof K.from_cid=="number")ve(K.from_cid,de.cid,"accent");else if(de.data.kind==="task"){let Pe=ne.find(Be=>Be.agent.tier==="orchestrator");Pe&&ve(Pe.agent.cid,de.cid,"accent")}break}default:break}}),rt=bt("delta",$=>{let de=ce.get($.cid);de&&z(de,"thinking\u2026")}),Ft=bt("conversation",$=>{let de=$;ve(de.from_cid,de.to_cid,"ok")}),wn=$=>{let de=Math.min(.05,($-Nt)/1e3);Nt=$;let be=$/1e3;te(de,$),R($),ae(be),P(be);for(let K of Le)K(be);h.update(),i.render(c,l),Y(),fe()};return i.setAnimationLoop(wn),()=>{_e.abort(),i.setAnimationLoop(null),xe.disconnect(),Re(),vt(),rt(),Ft(),h.dispose();for(let $ of[...ne])wt($);ne=[],ce.clear();for(let $ of pe.splice(0))Me($);wr(f),wr(d),wr(S),wr(g),wr(_),k.dispose(),L.dispose(),w.dispose(),T.dispose(),A.dispose(),D.dispose();for(let $ of Object.values(p))$.dispose();i.dispose(),n.classList.remove("floor-view"),document.body.classList.remove("floor-mode"),n.replaceChildren()}}var bf="http://www.w3.org/2000/svg";function qt(n,e,t={}){let i=document.createElementNS(bf,e);for(let r of Object.keys(t))i.setAttribute(r,String(t[r]));return n.appendChild(i),i}function zn(n,e,t,i,r,s="var(--sheet)",o={}){return qt(n,"rect",{x:e,y:t,width:i,height:r,fill:s,...o})}function Dl(n,e,t){e.forEach(([i,r],s)=>{let o=qt(n,"circle",{cx:i,cy:r,r:1.6,fill:t?"var(--ok)":"var(--paper)","stroke-width":1});t&&s===0&&qt(o,"animate",{attributeName:"opacity",values:"1;0.15;1",dur:"1.6s",repeatCount:"indefinite"})})}function ty(n,e){zn(n,8,18,68,44),zn(n,13,23,40,14,"var(--paper)");for(let t of[17,21,25])qt(n,"line",{x1:t,y1:27,x2:t,y2:33});zn(n,13,41,40,15,"var(--paper)");for(let t of[45,48,51,54])qt(n,"line",{x1:17,y1:t,x2:49,y2:t,"stroke-width":1}),qt(n,"line",{x1:58,y1:t,x2:73,y2:t,"stroke-width":1});return Dl(n,[[58,30],[63,30],[68,30],[73,30]],e),76}function ny(n,e){zn(n,26,8,42,76),qt(n,"polygon",{points:"68,8 76,3 76,79 68,84",fill:"var(--paper)"});for(let t of[30,36,42])qt(n,"line",{x1:70,y1:t-1.25,x2:75,y2:t-4.4,"stroke-width":1});zn(n,32,14,30,18,"var(--paper)"),qt(n,"line",{x1:36,y1:22,x2:58,y2:22,"stroke-width":1}),qt(n,"line",{x1:36,y1:26,x2:58,y2:26,"stroke-width":1});for(let t of[70,73,76,79])qt(n,"line",{x1:34,y1:t,x2:62,y2:t,"stroke-width":1});return qt(n,"circle",{cx:52,cy:60,r:3.5,fill:"var(--paper)"}),Dl(n,[[36,60],[42,60]],e),76}function iy(n,e){let t="#5a9a78";zn(n,10,30,70,40,"var(--sheet)",{stroke:t}),[[14,34],[76,34],[14,66],[76,66]].forEach(([r,s])=>{qt(n,"circle",{cx:r,cy:s,r:1.5,stroke:t,"stroke-width":1})}),zn(n,30,40,20,20,"var(--paper)"),qt(n,"rect",{x:36,y:46,width:8,height:8,"stroke-width":1});for(let r=0;r<10;r++){let s=52+r*2.2;qt(n,"circle",{cx:s,cy:34,r:.9,"stroke-width":.8}),qt(n,"circle",{cx:s,cy:37,r:.9,"stroke-width":.8})}return zn(n,69,48,11,9,"var(--paper)"),zn(n,69,58,11,9,"var(--paper)"),zn(n,54,60,13,10,"var(--paper)"),Dl(n,[[18,62],[23,62]],e),80}function ry(n,e){zn(n,10,30,66,30,"var(--sheet)",{rx:8}),qt(n,"line",{x1:18,y1:35,x2:66,y2:35,"stroke-width":1});for(let t of[50,53])qt(n,"line",{x1:20,y1:t,x2:50,y2:t,"stroke-width":1});return Dl(n,[[66,45]],e),76}var sy={server:ty,workstation:ny,edge:iy,apple:ry};function oy(n,e){let t=qt(n,"g");qt(t,"line",{x1:e,y1:46,x2:86,y2:46}),zn(t,86,36,28,24),qt(t,"circle",{cx:98,cy:46,r:6}),qt(t,"circle",{cx:98,cy:46,r:1.5}),zn(t,90,56,20,3,"var(--gold, #c9a227)",{stroke:"none"})}function Mf(n,e){let t=document.createElementNS(bf,"svg");t.setAttribute("viewBox","0 0 120 90"),t.setAttribute("class","device"),t.setAttribute("role","img"),t.setAttribute("aria-label",n),t.setAttribute("fill","none"),t.setAttribute("stroke","currentColor"),t.setAttribute("stroke-width","1.5");let i=sy[n](t,e.active);return e.gpu&&oy(t,i),t}var ay="http://www.w3.org/2000/svg",ly=2e3,Tf={server:"Server",workstation:"Workstation",edge:"Edge",apple:"Apple"},Sf=[["Run",!1],["Role",!1],["State",!1],["VMs",!0],["Started",!0],["",!1]];function Au(n){return/run|ready|up|active/i.test(n)?"ok":/start|boot|pend|creat|launch/i.test(n)?"info":/fail|err|crash|kill|dead/i.test(n)?"danger":"muted"}var Af=n=>n.some(e=>e.vms.some(t=>Au(t.state)==="ok")),cy=n=>(n.runs??[]).reduce((e,t)=>e+t.vms.length,0),Tu=n=>(n/1024).toFixed(1),Rf=(n,e,t)=>Math.min(t,Math.max(e,n));function wf(n,e){let t=document.createElementNS(ay,n);for(let[i,r]of Object.entries(e))t.setAttribute(i,r);return t}function uy(n){if(n<60)return`${Math.floor(n)}s`;let e=Math.floor(n/60);if(e<60)return`${e}m`;let t=Math.floor(e/60);return t<48?`${t}h ${e%60}m`:`${Math.floor(t/24)}d ${t%24}h`}function hy(n){let e=`${Tf[n.kind]} \xB7 ${n.cpus}-core ${n.arch} on ${n.backend.toUpperCase()} \xB7 ${Math.round(n.memory_mib/1024)} GiB memory`;return n.gpus.length?`${e} \xB7 ${n.gpus.map(t=>t.name).join(", ")}`:e}function dy(n){let i=document.createElement("canvas");i.className="mach-spark",i.width=240,i.height=72,i.setAttribute("aria-hidden","true");let r=i.getContext("2d");if(!r)return i;let s=getComputedStyle(document.documentElement);r.strokeStyle=s.getPropertyValue("--rule").trim()||"#999",r.lineWidth=1,r.beginPath(),r.moveTo(0,72-.5),r.lineTo(240,72-.5),r.stroke();let o=n.slice(-60);return o.length<2||(r.strokeStyle=s.getPropertyValue("--ink").trim()||"#111",r.lineWidth=2,r.beginPath(),o.forEach((a,c)=>{let l=c/(o.length-1)*240,h=68-Rf(a.cpu_percent,0,100)/100*64;c===0?r.moveTo(l,h):r.lineTo(l,h)}),r.stroke()),i}function fy(n){let e=v("div.mach-bar-fill",null);return e.style.width=`${Rf(n.mem_used_mib/Math.max(n.mem_total_mib,1)*100,0,100)}%`,v("div.mach-bar",null,e)}function Nl(n,e,t,i){let r=v("div.mach-figure",null,e);return e.length>8&&r.classList.add("long"),v("div.mach-tally",null,v("div.mach-tally-label",null,n),r,i??null,t===null?null:v("div.mach-caption",null,t))}function Ef(n,e,t,i){let r=wf("svg",{class:"mach-wires",viewBox:"0 0 100 100",preserveAspectRatio:"none","aria-hidden":"true"}),s=v("div.mach-wires-box",null,r),o=v("div.mach-fleet");o.style.gridTemplateRows=`repeat(${Math.max(e.length,1)}, minmax(44px, 1fr))`,e.length||o.append(ht("machine","No machines.")),e.forEach((c,l)=>{let h=Af(c.runs??[]),m=(l+.5)/e.length*100;r.append(wf("line",{x1:"0",y1:"50",x2:"100",y2:String(m),class:h?"mach-wire live":"mach-wire","vector-effect":"non-scaling-stroke"}));let u=v("div.mach-wire-label",null,`${c.agents??0} agents \xB7 ${cy(c)} VMs`);u.style.top=`${(50+m)/2}%`,s.append(u);let g=v("span.mach-mark",null);h&&g.classList.add("live"),c.reachable===!1&&g.classList.add("down");let _=c.reachable===!1?"unreachable":h?"running":"idle",x=v("div.mach-block",{role:"button",tabindex:"0"},v("div.mach-row",null,g,v("span.mach-name",null,c.name)),v("div.faint",null,`${Tf[c.kind]} \xB7 ${_}`));x.classList.toggle("selected",c.id===t),x.addEventListener("click",()=>i(c.id)),x.addEventListener("keydown",f=>{f.key==="Enter"&&i(c.id)}),o.append(x)});let a=v("div.mach-plane",null,v("div.mach-caps",null,"Control plane"),v("div.faint",null,n?`${n.agents.total} agents \xB7 ${n.agents.working} working`:"Unreachable"),v("div.mach-chips",null,ot("apid",n?"ok":"danger",!0),ot("hub",n?.hub?"ok":"muted",!0)));return v("div.mach-topo",null,a,s,o)}function Cf(n,e){let t=v("div"),i=v("div");n.append(v("div.view-head",null,v("h1",null,"Machines")),t,i);let r=new Set,s=new Set,o=new Set,a=new Map,c=!0,l,h=[],m=null,u=null,g=(p,M)=>M.length?v("div.vms",null,M.map(y=>{let C=`${p}/${y.index}`,k=null;if(y.serial){let L=v("details",null,v("summary",null,"Serial"),v("pre.pre",null,y.serial));L.open=s.has(C),L.addEventListener("toggle",()=>L.open?s.add(C):s.delete(C)),k=L}return v("div.vm",null,v("div.row",null,v("strong",null,`VM ${y.index}`),ot(y.state,Au(y.state),!0),v("span.faint",null,`guest CID ${y.guest_cid??"\u2014"}`)),k)})):v("p.faint",null,"No VMs."),_=p=>{let M=a.get(p.id),y=v("td",{colspan:Sf.length},M?g(p.id,M.vms):v("p.faint",null,"Loading\u2026")),C=v("tr.detail",null,y);return Oe.run(p.id).then(k=>{a.set(p.id,k),C.isConnected&&Ue(y,g(p.id,k.vms))},k=>{C.isConnected&&Ue(y,v("p.faint",null,k instanceof Error?k.message:String(k)))}),C},x=(p,M)=>{let y=M.nextElementSibling;y&&y.classList.contains("detail")?(y.remove(),r.delete(p.id)):(r.add(p.id),M.after(_(p)))},f=async p=>{await dt(()=>Oe.killRun(p),`Killed run ${p}`),c&&w()},d=p=>{let M=v("tbody");for(let y of p){let C=We("Kill",L=>{L.stopPropagation(),f(y.id)},{kind:"danger",size:"sm",icon:"stop"}),k=v("tr.clickable",null,v("td",null,y.id),v("td",null,y.role??"fleet"),v("td",null,ot(y.state,Au(y.state),!0)),v("td",{class:"num"},`${y.vms.length} VMs`),v("td",{class:"num"},`started ${Wt(y.created_at)}`),v("td",{class:"num"},C));k.addEventListener("click",()=>x(y,k)),M.append(k),r.has(y.id)&&M.append(_(y))}return v("table.table.mach-runs",null,v("thead",null,v("tr",null,Sf.map(([y,C])=>v("th",{class:C?"num":""},y)))),M)},S=p=>{let M=p.runs??[],y=Mf(p.kind,{gpu:p.gpus.length>0,active:Af(M)});y.classList.add("mach-dev");let C=p.stats,k=C?v("div.mach-live",null,v("div.mach-tallies",null,Nl("CPU",`${Math.round(C.cpu_percent)}%`,null,dy(C.history)),Nl("Memory",`${Tu(C.mem_used_mib)} / ${Tu(C.mem_total_mib)}`,"GiB used of GiB",fy(C)),Nl("Load",`${C.load[0].toFixed(1)} \xB7 ${C.load[1].toFixed(1)} \xB7 ${C.load[2].toFixed(1)}`,"1 / 5 / 15 min"),Nl("Agents' VMs",String(C.vms_running),`${Tu(C.guest_mem_mib)} GiB reserved`)),v("p.faint",null,`Up ${uy(C.uptime_s)}`)):v("p.mach-none",null,"No live stats."),L=v("dl.kv"),U=[["Class",p.class],["Arch",p.arch],["Backend",p.backend],["Irqchip",p.irqchip],["vsock",p.vsock_mode],["noded port",p.noded_port===void 0?"\u2014":String(p.noded_port)]];for(let[H,q]of U)L.append(v("dt",null,H),v("dd",null,q));let F=v("details.mach-tech",null,v("summary",null,"Technical details"),L);return F.open=o.has(p.id),F.addEventListener("toggle",()=>F.open?o.add(p.id):o.delete(p.id)),v("section.mach-section",{id:`mach-${p.id}`},v("div.mach-figure-box",null,y),v("div.mach-body",null,v("h2.mach-heading",null,p.name),v("p.mach-sentence",null,hy(p)),k,v("div.mach-caps",null,"Runs"),M.length?d(M):v("p.faint",null,"No runs."),F))},P=p=>{u=p,t.replaceChildren(Ef(m,h,u,P)),document.getElementById(`mach-${p}`)?.scrollIntoView({behavior:"smooth",block:"start"})},w=async()=>{try{let[p,M]=await Promise.all([Oe.overview().catch(()=>null),Oe.machines()]);if(!c)return;m=p,h=M,M.some(C=>C.id===u)||(u=M[0]?.id??null);let y=new Set(M.flatMap(C=>(C.runs??[]).map(k=>k.id)));for(let C of Array.from(r))y.has(C)||r.delete(C);t.replaceChildren(Ef(p,M,u,P)),i.replaceChildren(...M.length?M.map(S):[ht("machine","No machines.")])}catch(p){if(!c)return;t.replaceChildren(),i.replaceChildren(ht("alert","Machines unreachable",p instanceof Error?p.message:String(p)))}},T=()=>{l!==void 0&&clearTimeout(l),l=setTimeout(()=>{l=void 0,w()},500)},A=[bt("run",T),bt("vm",T)],D=setInterval(()=>{w()},ly);return w(),()=>{c=!1,A.forEach(p=>p()),clearInterval(D),l!==void 0&&clearTimeout(l)}}var py=["default","low","medium","high","xhigh","max"],Pf="models-policy-known";function If(n){let e=!0;return Ue(n,v("p.faint",null,"Loading\u2026")),dt(()=>Promise.all([Oe.settings(),Oe.models()])).then(t=>{if(e){if(!t){Ue(n,ht("alert","Models unavailable","apid did not answer. Reload to try again."));return}my(n,t[0],t[1])}}),()=>{e=!1}}function my(n,e,t){let i=e,r=Pu(e),s=e.dev_models===!0,o=new Set(t.models.map(p=>`${p.provider}/${p.id}`)),a=new Set,c=!1,l=We("Save",()=>{_()},{kind:"primary",icon:"check"}),h=v("datalist",{id:Pf},...t.models.map(p=>v("option",{value:`${p.provider}/${p.id}`},p.name??`${p.provider}/${p.id}`))),m=v("div.stack.models-policy");Ue(n,v("div.policy-bar",null,l),h,m);function u(){l.disabled=a.size>0||JSON.stringify(r)===JSON.stringify(i)}function g(){a.clear(),Ue(m,f(),P(),T(),D()),u()}async function _(){l.disabled=!0;let p=await dt(()=>Oe.saveSettings(r),"Policy saved");p?(i=Pu(p),r=Pu(p),g()):u()}function x(p,M,y){y?a.add(M):a.delete(M),p.classList.toggle("invalid",y),u()}function f(){return zt("Model policy",v("div.stack",null,v("p.policy-lead",null,"Agents may only run the models on their tier's list."),v("div.policy-cols",null,d("orchestrator","Orchestrator"),d("worker","Workers"))),{actions:s?[ot("Development models enabled \u2014 policy relaxed","warn")]:[]})}function d(p,M){let C=r.model_policy[p].map((U,F)=>S(p,U,F)),k=Zt({placeholder:"provider/model",list:Pf,autocomplete:"off","aria-label":`${M}: model to add`}),L=()=>{let U=k.value.trim();if(U!==""){if(!/^[^/\s]+\/\S+$/.test(Ru(U).base)){nn("Use provider/model, with an optional @effort","error");return}r.model_policy[p].includes(U)||(r.model_policy[p]=[...r.model_policy[p],U]),g()}};return k.addEventListener("keydown",U=>{U.key==="Enter"&&(U.preventDefault(),L())}),v("section.policy-col",null,v("h3",null,M),v("ul.policy-list",null,...C.length?C:[v("li.faint",null,"No models")]),v("div.policy-add",null,k,We("Add model",L,{size:"sm"})))}function S(p,M,y){let{base:C,effort:k}=Ru(M),L=o.has(C),U=ci(L?"ok":"muted");U.title=L?"Listed by its provider":"Not listed by its provider";let F=k===null?null:ot("effort pinned","accent");F&&(F.title=`@${k}`);let H=We("",()=>{r.model_policy[p]=r.model_policy[p].filter((q,ee)=>ee!==y),g()},{kind:"ghost",size:"sm",icon:"x",title:`Remove ${M}`});return v("li.policy-row",null,U,v("span.mono",{title:M},C),F,H)}function P(){let p=fn(Cu(r.model_policy.orchestrator,r.orchestrator_model),r.orchestrator_model);p.addEventListener("change",()=>{r.orchestrator_model=p.value,u()});let M=fn(Cu(r.model_policy.worker,r.default_model),r.default_model);M.addEventListener("change",()=>{r.default_model=M.value,u()});let y=Array.from(new Set([...ut.get().filter(k=>k.tier!=="orchestrator"&&k.role!=="orchestrator").map(k=>k.role),...Object.keys(r.role_models)])).sort(),C=y.length?rr([{label:"Role",cell:k=>v("span.mono",null,k)},{label:"Model",cell:k=>w(k)}],y):ht("agents","No roles connected");return zt("Defaults",v("div.stack",null,gt("Orchestrator model",p),gt("Worker default",M),v("h3",null,"Per role"),C))}function w(p){let M=r.role_models[p]??"",y=fn(Cu(r.model_policy.worker,M===""?null:M,{value:"",label:"Worker default"}),M);return y.addEventListener("change",()=>{y.value===""?delete r.role_models[p]:r.role_models[p]=y.value,u()}),y}function T(){let p=A("max_turns",1,200,r.max_turns,k=>{r.max_turns=k}),M=A("max_tokens",0,1e9,r.max_tokens,k=>{r.max_tokens=k}),y=fn(py.map(k=>({value:k,label:k})),r.effort??"default");y.addEventListener("change",()=>{r.effort=y.value==="default"?null:y.value,u()});let C=Kn({rows:5,spellcheck:"false",placeholder:"api.example.com"});return C.value=r.fetch_allow.join(`
`),C.addEventListener("input",()=>{r.fetch_allow=C.value.split(`
`).map(k=>k.trim()).filter(k=>k!==""),u()}),zt("Limits",v("div.stack",null,gt("Max turns",p),gt("Max tokens (0 = provider default)",M),gt("Effort",y),gt("Fetch allowlist",C)))}function A(p,M,y,C,k){let L=Zt({type:"number",min:M,max:y,step:1,value:C});return L.addEventListener("input",()=>{let U=Number(L.value),F=L.value.trim()!==""&&Number.isInteger(U)&&U>=M&&U<=y;F&&k(U),x(L,p,!F)}),L}function D(){let p=r.model_policy.orchestrator.concat(r.model_policy.worker).map(L=>Ru(L).base),M=Array.from(new Set([...p,...Object.keys(r.prices)])).sort(),y=(L,U)=>{let F=r.prices[L],H=Zt({type:"number",min:0,step:"any",value:F?F[U]:0});return H.addEventListener("input",()=>{let q=Number(H.value),ee=H.value.trim()!==""&&Number.isFinite(q)&&q>=0;if(ee){let J=r.prices[L]??{input:0,output:0};J[U]=q,r.prices[L]=J}x(H,`price:${L}:${U}`,!ee)}),H},C=M.length?rr([{label:"Model",cell:L=>v("span.mono",null,L)},{label:"Input (credits / 1M)",num:!0,cell:L=>y(L,"input")},{label:"Output (credits / 1M)",num:!0,cell:L=>y(L,"output")}],M):ht("models","No models listed"),k=v("details.prices",null,v("summary",null,"Models"),C);return k.open=c,k.addEventListener("toggle",()=>{c=k.open}),zt("Prices",k)}g()}function Ru(n){let e=n.lastIndexOf("@");return e>0?{base:n.slice(0,e),effort:n.slice(e+1)}:{base:n,effort:null}}function Cu(n,e,t){let r=(e!==null&&e!==""&&!n.includes(e)?[...n,e]:n).map(s=>({value:s,label:n.includes(s)?s:`${s} (not on the list)`}));return t?[t,...r]:r}function Pu(n){return JSON.parse(JSON.stringify(n))}var gy=["Configured with a key","Needs a key","Local"],xy={env:["Key set","ok"],secret:["Key set","ok"],none:["No key needed","muted"],command:["Command","info"],missing:["Key missing","warn"]};function Lf(n,e){return e.find(t=>t.id===n.id)??e.find(t=>t.kind===n.kind)}function vy(n){return n.credential==="missing"?"Needs a key":n.credential==="none"?"Local":"Configured with a key"}function _y(n){return n.split(",").map(e=>e.trim()).filter(Boolean)}function kf(n){let e=[],t=[],i=v("div.prov-body",null,v("p.faint",null,"Loading providers\u2026")),r=We("Add provider",()=>_(),{kind:"primary"});n.append(v("div.prov-bar",null,r),i);let s=async()=>{let[x,f]=await Promise.all([dt(()=>Oe.providerKinds()),dt(()=>Oe.providers())]);x&&(e=x),f&&(t=f),o()},o=()=>{if(!t.length){Ue(i,ht("models","No providers yet"));return}let x={"Configured with a key":[],"Needs a key":[],Local:[]};for(let d of t)x[vy(d)].push(d);let f=gy.filter(d=>x[d].length>0).map(d=>v("section.prov-group",null,v("h2.prov-group-title",null,d),v("div.prov-grid",null,x[d].map(a))));Ue(i,...f)},a=x=>{let f=Lf(x,e),[d,S]=xy[x.credential],P=v("div.prov-models");P.hidden=!0;let w=!1,T=We("",async()=>{if(P.hidden=!P.hidden,P.hidden||w)return;let A=await dt(()=>Oe.providerModels(x.id));A&&(w=!0,Ue(P,A.length?v("ul.prov-model-list",null,A.map(D=>v("li.mono",null,D.id))):v("p.faint",null,"No models listed")))},{size:"sm",kind:"ghost",icon:"models",title:"Models"});return v("article.prov-card",null,v("header.prov-head",null,Wl(f?.brand??"mock",!0),v("div.prov-title",null,v("h3.truncate",null,f?.label??x.id),v("span.faint.mono.truncate",{title:x.base_url??""},`${x.id} \xB7 ${x.base_url??"\u2014"}`))),v("div.prov-meta",null,ot(d,S,!0),v("span.faint",null,`${x.models.length} ${x.models.length===1?"model":"models"}`)),v("div.prov-actions",null,We("Test",()=>{c(x)},{size:"sm"}),We(x.credential==="missing"?"Add key":"Key",()=>l(x),{size:"sm"}),v("span.prov-spacer"),T,We("",()=>u(x),{size:"sm",kind:"ghost",icon:"edit",title:"Edit"}),We("",()=>h(x),{size:"sm",kind:"ghost",icon:"trash",title:"Remove"})),P)},c=async x=>{let f=await dt(()=>Oe.testProvider(x.id));f&&(f.ok?nn(`OK \xB7 ${f.latency_ms} ms \xB7 ${f.model}`):nn(f.error??"Test failed","error"))},l=x=>{let f=()=>{},d=Zt({type:"password",autocomplete:"new-password",placeholder:"API key"}),S=We("Save key",async()=>{let P=d.value.trim();if(!P)return;S.disabled=!0;let w=await dt(()=>Oe.setSecret(x.id,P),"Key saved");S.disabled=!1,w!==void 0&&(d.value="",f(),await s())},{kind:"primary"});d.addEventListener("keydown",P=>{P.key==="Enter"&&S.click()}),f=Cn(`Set key \xB7 ${x.id}`,v("div.stack",null,gt("API key",d),v("div.prov-row",null,v("span.prov-spacer"),S)))},h=x=>{let f=()=>{},d=We("Remove provider",async()=>{await dt(()=>Oe.removeProvider(x.id),`Removed ${x.id}`)!==void 0&&(f(),await s())},{kind:"danger"}),S=We("Cancel",()=>f(),{});f=Cn(`Remove ${x.id}?`,v("div.stack",null,v("div.prov-row",null,v("span.prov-spacer"),S,d)))},m=(x,f,d,S)=>{let P=()=>{},w=Zt({value:f.id,placeholder:"provider id",autocomplete:"off",disabled:!f.idEditable}),T=Zt({value:f.base_url,placeholder:"https://",autocomplete:"off"}),A=typeof f.auth=="object"&&"env"in f.auth?f.auth.env:null,D=A===null?null:Zt({value:A,placeholder:"ENV_VAR_NAME",autocomplete:"off"}),p=f.models===null?null:Zt({value:f.models.join(", "),autocomplete:"off"}),M=f.keyField?Zt({type:"password",autocomplete:"new-password",placeholder:"Optional"}):null,y=[gt("Provider id",w),gt("Base URL",T)];D&&y.push(gt("Auth env var",D)),p&&y.push(gt("Models",p,"Comma-separated")),M&&y.push(gt("API key",M,"Optional"));let C=We(d,async()=>{let k=w.value.trim();if(!k){nn("Give the provider an id","error");return}C.disabled=!0;let L=await S({id:k,base_url:T.value.trim()||null,auth:D?{env:D.value.trim()}:f.auth,models:p?_y(p.value):null,key:M?.value.trim()??""});C.disabled=!1,L&&P()},{kind:"primary"});P=Cn(x,v("div.stack",null,f.notes?v("p.faint",null,f.notes):null,...y,v("div.prov-row",null,v("span.prov-spacer"),C)))},u=x=>{let f=Lf(x,e);m(`Edit ${x.id}`,{id:x.id,base_url:x.base_url??"",auth:x.auth,models:x.models,idEditable:!1,keyField:!1,notes:f?.notes},"Save",async d=>await dt(()=>Oe.updateProvider({id:x.id,kind:x.kind,base_url:d.base_url,auth:d.auth,models:d.models??x.models,options:x.options??void 0,extra_headers:x.extra_headers}),"Saved")===void 0?!1:(await s(),!0))},g=x=>{m(x.label,{id:x.id,base_url:x.base_url??"",auth:x.auth,models:null,idEditable:!0,keyField:!0,notes:x.notes},"Add provider",async f=>await dt(()=>Oe.addProvider({id:f.id,kind:x.kind,base_url:f.base_url,auth:f.auth,models:x.models,options:x.options}),`Added ${f.id}`)===void 0?!1:(f.key&&await dt(()=>Oe.setSecret(f.id,f.key),"Key saved"),await s(),!0))},_=()=>{let x=()=>{},f=new Set(t.map(A=>A.id)),d=e.filter(A=>!f.has(A.id)),S=Zt({type:"search",placeholder:"Search providers",autocomplete:"off"}),P=v("div.prov-kind-grid"),w=A=>{x(),g(A)},T=()=>{let A=S.value.trim().toLowerCase(),p=d.filter(M=>!A||`${M.label} ${M.id} ${M.kind}`.toLowerCase().includes(A)).map(M=>{let y=v("button.prov-kind",{type:"button"},Wl(M.brand,!0),v("span.truncate",null,M.label));return y.addEventListener("click",()=>w(M)),y});Ue(P,...p.length?p:[v("p.faint",null,d.length?"No matches":"Every provider is already configured")])};S.addEventListener("input",T),x=Cn("Add provider",v("div.stack",null,S,P)),T()};return s(),()=>{}}function Df(n,e){let t=e[0]==="policy"?"policy":"providers",i=v("div.stack");return n.append(v("div.view-head",null,v("h1",null,"Models")),qu([{key:"providers",label:"Providers"},{key:"policy",label:"Policy & defaults"}],t,r=>_n("models",...r==="policy"?["policy"]:[])),i),t==="policy"?If(i):kf(i)}var yy=40;function Nf(n){let e=v("div.grid.cols-4"),t=v("div"),i=v("ol.feed"),r=v("div");n.append(v("div.view-head",null,v("h1",null,"Overview")),e,v("div.grid.cols-2.overview-main",null,t,zt("Activity",i,{flush:!0})),zt("Fleet",r,{flush:!0}));let s=[ir.watch(o=>{if(!o)return Ue(e,ht("alert","apid is unreachable","Start apid, then reload."));let a=(o.tasks.open??0)+(o.tasks.claimed??0);Ue(e,Zn("Agents",o.agents.total,`${o.agents.working} working \xB7 ${o.agents.offline} offline`),Zn("Open tasks",a,`${o.tasks.done??0} done`),Zn("Worker model",o.default_model.split("/").pop()??"\u2014",o.default_model.split("/")[0]),Zn("Orchestrator model",o.orchestrator_model.split("/").pop()??"\u2014",o.orchestrator_model.split("/")[0]))}),ut.watch(o=>{by(t,o.find(a=>a.tier==="orchestrator")),My(r,o)}),bt("transcript",o=>Sy(i,o))];return Ue(i,v("li.feed-empty.faint",null,"Waiting for activity\u2026")),()=>s.forEach(o=>o())}function by(n,e){if(!e){Ue(n,zt("Orchestrator",ht("orchestrator","No orchestrator connected","Add the orchestrator role to the fleet to delegate work by goal.")));return}let t=Kn({placeholder:"Tell the orchestrator what you want done\u2026",rows:3}),i=We("Send",()=>r(),{kind:"primary",icon:"send"}),r=async()=>{let o=t.value.trim();if(!o)return;i.disabled=!0;let a=await dt(()=>Oe.chat(e.cid,o),"Sent to the orchestrator");i.disabled=!1,a&&(t.value="")};t.addEventListener("keydown",o=>{o.key==="Enter"&&(o.metaKey||o.ctrlKey)&&r()});let s=e.status==="working"?ot("Working","info",!0):e.status==="idle"?ot("Idle","ok",!0):ot("Offline","muted",!0);Ue(n,zt(v("div.row",null,li("orchestrator"),v("h2",null,pn(e)),v("span.faint",null,yn(e.role)),s),v("div.stack",null,v("p.muted",null,`Plans the work and delegates it to the fleet \xB7 ${e.model}`),t,v("div.row",null,v("span.faint",null,"Ctrl+Enter to send"),v("span",{style:"flex:1"}),v("a",{href:vn("agents",e.cid)},"Open transcript"),i))))}function My(n,e){if(!e.length)return Ue(n,ht("agents","No agents yet","Agents appear here as their VMs connect."));Ue(n,v("div.fleet",null,e.map(t=>{let i=t.status==="working"?"info":t.status==="idle"?"ok":"muted",r=v("button.fleet-tile",{type:"button",title:`${pn(t)} \xB7 ${yn(t.role)} \xB7 ${t.model}`},v("span.row",null,v("span.dot",{class:i}),v("strong.truncate",null,pn(t))),v("span.faint.truncate",null,t.work?Kt(t.work.prompt,60):t.status==="offline"?"offline":yn(t.role)));return r.addEventListener("click",()=>_n("agents",t.cid)),r})))}function Sy(n,e){let t=wy(e);if(!t)return;n.querySelector(".feed-empty")?.remove();let i=ut.get().find(o=>o.cid===e.cid),r=i?pn(i):`agent ${e.cid}`,s=v("li",null,v("span.feed-time.faint",null,Wt(e.at_ms)),v("a",{href:vn("agents",e.cid)},r),v("span.muted",null,t));for(n.prepend(s);n.children.length>yy;)n.lastElementChild?.remove()}function wy(n){let e=n.data;switch(n.kind){case"work_start":return`started ${String(e.kind)}: ${Kt(String(e.prompt??""),80)}`;case"work_end":return`finished (${String(e.status)})`;case"tool_request":return`called ${String(e.name)}`;case"error":return`error: ${Kt(String(e.message??""),80)}`;default:return null}}var Ey=[{label:"Open",idle:"Nothing open."},{label:"In progress",idle:"Nothing in progress."},{label:"Done",idle:"Nothing finished yet."},{label:"Failed or withdrawn",idle:"Nothing failed or withdrawn."}],Ty=new Set(["failed","withdrawn","cancelled"]),Ay=new Set(["work_start","tool_request","work_end","error"]);function Ul(n){return n==="open"?0:n==="done"?2:Ty.has(n)?3:1}function Ry(n){switch(Ul(n)){case 0:return"info";case 1:return"accent";case 2:return"ok";default:return n==="withdrawn"?"muted":"danger"}}var gs=n=>typeof n=="string"||typeof n=="number"?String(n):"",Uf=n=>n.split(`
`).find(e=>e.trim())?.trim()??"Untitled task",Iu=n=>n.replace(/_/g," ").replace(/^./,e=>e.toUpperCase()),Cy=n=>{let e=ut.get().find(t=>t.cid===n);return e?pn(e):`agent ${n}`},Ff=()=>ut.get().filter(n=>n.status==="idle"&&n.tier==="worker"),Py=n=>ut.get().find(e=>e.cid===n)?.status??"offline",Fl=n=>String(n).padStart(2,"0"),Of=n=>{let e=Math.max(0,Math.floor(n/1e3));return`${Fl(Math.floor(e/60))}:${Fl(e%60)}`},Iy=n=>{let e=Math.max(0,Math.round(n/1e3));return e<60?`${e}s`:e<3600?`${Math.floor(e/60)}m ${Fl(e%60)}s`:`${Math.floor(e/3600)}h ${Fl(Math.floor(e%3600/60))}m`},Ly=n=>new Date(n).toLocaleTimeString([],{hour:"2-digit",minute:"2-digit",second:"2-digit"}),Lu=n=>gs(n.data.tool)||gs(n.data.name)||"tool";function zf(n){let e=n.data.input??{};return Lu(n).toLowerCase()==="bash"&&typeof e.command=="string"?e.command:null}function ky(n){switch(n.kind){case"tool_request":{let e=zf(n);return e!==null?`$ ${e}`:`\u2192 ${Lu(n)}`}case"model_request":return"thinking\u2026";case"work_start":return"starting\u2026";case"work_end":return"wrapping up\u2026";default:return null}}function Dy(n){switch(n.kind){case"work_start":return`Started \xB7 ${gs(n.data.kind)||"work"}`;case"tool_request":{let e=zf(n);return e!==null?`$ ${e}`:`\u2192 ${Lu(n)}`}case"work_end":{let e=gs(n.data.status);return e?`Finished \xB7 ${e}`:"Finished"}default:return gs(n.data.message)||gs(n.data.error)||"Error"}}function Ny(n,e){let t=new Set(e.filter(i=>i.kind==="work_start"&&((i.data.origin??{}).task===n||i.data.task===n)).map(i=>i.work));return e.filter(i=>t.has(i.work)&&Ay.has(i.kind))}function Bf(n){if(n===null)return v("span.faint",null,"Unassigned");let e=v("a",{href:vn("agents",n)},Cy(n));return e.addEventListener("click",t=>t.stopPropagation()),e}var Uy=n=>v("li",{class:`k-${n.kind}`},v("time",null,Ly(n.at_ms)),v("span",null,Dy(n)));function Hf(n,e){let t=e[0]??"",i=[],r=null,s=null,o=!1,a,c=0,l,h=new Map,m=new Map,u=new Set,g=[],_=()=>{Ue(n,t?D():A())},x=async y=>{if(s=null,y.agent_cid===null){s=[];return}try{s=Ny(y.id,await Oe.transcript(y.agent_cid))}catch{s=[]}},f=async()=>{let y=++c;try{if(t){let C=await Oe.task(t);if(y!==c||(r=C,o=!1,await x(C),y!==c))return}else{let C=await Oe.tasks();y===c&&(i=C)}}catch{y===c&&t&&(r=null,o=!0)}y===c&&_()},d=()=>{clearTimeout(a),a=setTimeout(()=>{f()},300)},S=()=>{let y=Zt({required:!0,placeholder:"What should get done?"}),C=Kn({rows:4}),k=Kn({rows:3}),L=Array.from(new Set(ut.get().filter(ne=>ne.tier==="worker").map(ne=>ne.role).filter(Boolean))).sort(),U=fn([{value:"",label:"Any"},...L.map(ne=>({value:ne,label:ne}))]),F=Zt({type:"number",min:0,step:1,value:100}),H=fn([{value:"",label:"Let the scheduler choose"},...Ff().map(ne=>({value:String(ne.cid),label:pn(ne)}))]),q=v("button.btn.primary",{type:"submit"},li("plus"),"Create task"),ee=v("form.stack",null,gt("Title",y),gt("Details",C),gt("Acceptance",k),v("div.grid.cols-2",null,gt("Role",U),gt("Budget",F)),gt("Assign to",H),v("div.row",null,v("span",{style:"flex:1"}),q));ee.addEventListener("submit",async ne=>{ne.preventDefault();let ce=y.value.trim();if(!ce){y.focus();return}let pe=C.value.trim();q.disabled=!0;let me=await dt(()=>Oe.createTask({goal:pe?`${ce}

${pe}`:ce,acceptance:k.value.trim()||void 0,role:U.value||void 0,budget:F.value===""?100:Number(F.value),assignee:H.value?Number(H.value):void 0}),"Task created");q.disabled=!1,me&&(J(),_n("tasks",me.id))});let J=Cn("New task",ee,()=>{l=void 0});l=J};function P(y){let C=U=>y.filter(F=>Ul(F.state)===U).length,k=ut.get().filter(U=>U.status==="working").length,L=(U,F)=>v("div.tk-fig",null,v("b",null,String(F)),v("span",null,U));return v("div.tk-summary",null,L("Open",C(0)),L("In progress",C(1)),L("Done",C(2)),L("Failed",C(3)),L("Agents working",k))}function w(y){let C=Ul(y.state),k=m.get(y.id),L=["task-card",`lane-${C}`];u.has(y.id)||L.push("enter"),k!==void 0&&k!==y.state&&L.push("changed"),u.add(y.id),m.set(y.id,y.state);let U=y.poster,F=v("div.task-who",null,v("span.task-dot",{class:`s-${Py(y.agent_cid)}`}),Bf(y.agent_cid),U?v("span.faint",null,`by ${U}`):null),H;if(C===1){let J=v("span.task-elapsed",null,Of(Date.now()-y.updated_at));g.push({el:J,since:y.updated_at});let ne=y.agent_cid!==null&&h.get(y.agent_cid)||"waiting for the first step\u2026";H=v("div.task-live",null,v("div.task-stripe"),v("div.task-activity",{"data-activity":String(y.agent_cid??"")},ne),v("div.task-meta",null,v("span.faint",null,"elapsed"),J))}else C===2?H=v("div",null,y.note?v("p.task-note",null,Kt(y.note,120)):null,v("div.task-meta",null,v("span.faint",null,`took ${Iy(y.updated_at-y.created_at)}`),v("span.faint",null,Wt(y.updated_at)))):C===3?H=v("div",null,v("p.task-reason",null,y.outcome||y.note||Iu(y.state)),v("div.task-meta",null,v("span.faint",null,Iu(y.state)),v("span.faint",null,Wt(y.updated_at)))):H=v("div.task-meta",null,v("span.faint",null,`budget ${y.budget}`),v("span.faint",null,`created ${Wt(y.created_at)}`));let q=v("div",{class:L.join(" "),role:"link",tabindex:0},v("div.task-top",null,v("span.task-id",null,y.id),v("span.task-role",null,y.role?yn(y.role):"Any role")),v("strong.task-title",null,Uf(y.goal)),F,H),ee=()=>_n("tasks",y.id);return q.addEventListener("click",ee),q.addEventListener("keydown",J=>{J.key==="Enter"&&ee()}),q}function T(y){g=[];let C=y.slice().sort((k,L)=>L.updated_at-k.updated_at);return v("div.tk-board-wrap",null,v("div.tk-board",null,Ey.map((k,L)=>{let U=C.filter(F=>Ul(F.state)===L);return v("section.tk-lane",{class:`lane-${L}`},v("header.tk-lane-head",null,v("span.tk-mark",{class:`mark-${L}`}),v("h2",null,k.label),v("span.tk-count",null,String(U.length))),v("div.tk-lane-body",null,U.length?U.map(w):v("p.tk-empty",null,k.idle)))})))}function A(){return v("div.tasks-view",null,v("div.view-head",null,v("h1",null,"Tasks"),v("div.actions",null,We("New task",S,{kind:"primary",icon:"plus"}))),P(i),T(i))}function D(){let y=v("a.back",{href:vn("tasks")},li("chevronRight"),"Tasks");if(o)return v("div.tasks-view",null,y,v("p.tk-empty",null,"Task not found."));let C=r;if(!C)return v("div.tasks-view",null,y);let k=C.state==="open",L=fn([{value:"",label:"Assign to\u2026"},...Ff().map(ne=>({value:String(ne.cid),label:pn(ne)}))],"");L.addEventListener("change",async()=>{L.value&&(L.disabled=!0,await dt(()=>Oe.assign(Number(L.value),C.id),"Assigned"),L.disabled=!1,f())});let U=We("Withdraw",async()=>{U.disabled=!0,await dt(()=>Oe.withdrawTask(C.id),"Task withdrawn")?f():U.disabled=!1},{kind:"danger",icon:"x"}),F=v("dl.kv",null,v("dt",null,"State"),v("dd",null,ot(Iu(C.state),Ry(C.state),!0)),v("dt",null,"Role"),v("dd",null,C.role?yn(C.role):"Any role"),v("dt",null,"Assignee"),v("dd",null,Bf(C.agent_cid)),v("dt",null,"Budget"),v("dd",null,String(C.budget)),v("dt",null,"Created"),v("dd",null,Wt(C.created_at)),v("dt",null,"Updated"),v("dd",null,Wt(C.updated_at))),H=s,q=H===null?v("p.tk-empty",null,"Loading\u2026"):H.length===0?v("p.tk-empty",null,C.agent_cid===null?"Not yet assigned.":"No steps recorded yet."):v("ol.tk-tl",null,H.map(Uy)),ee=v("div.tk-main",null,v("section.tk-block",null,v("h3.tk-label",null,"Goal"),v("div.tk-goal",null,C.goal)),C.acceptance?v("section.tk-block",null,v("h3.tk-label",null,"Acceptance"),v("div.tk-prose",null,C.acceptance)):null,C.note?v("section.tk-block",null,v("h3.tk-label",null,"Result"),v("div.tk-result",null,C.note)):null),J=v("aside.tk-side",null,v("section.tk-block",null,v("h3.tk-label",null,"Facts"),F,k?L:null,k?v("div.row",null,v("span",{style:"flex:1"}),U):null),v("section.tk-block",null,v("h3.tk-label",null,"Timeline"),q));return v("div.tasks-view",null,y,v("div.view-head",null,v("div.tk-head",null,v("span.task-id",null,C.id),v("h1",null,Uf(C.goal)))),v("div.tk-cols",null,ee,J))}_();let p=setInterval(()=>{let y=Date.now();for(let C of g)C.el.textContent=Of(y-C.since)},1e3),M=[bt("task",d),bt("work",d),bt("transcript",y=>{let C=y;if(t){r&&C.cid===r.agent_cid&&d();return}let k=ky(C);k!==null&&(h.set(C.cid,k),n.querySelectorAll(`[data-activity="${C.cid}"]`).forEach(L=>{L.textContent=k}))}),ut.watch(()=>_())];return f(),()=>{clearTimeout(a),clearInterval(p),M.forEach(y=>y()),g=[],l?.()}}var _i=n=>n.toLocaleString("en-US"),ku=n=>n instanceof Error?n.message:String(n),Fy={mint:"ok",spend:"warn",payment:"accent",refund:"info",transfer:"info",fine:"danger"},Vf=n=>n.postings.reduce((e,t)=>t.amount>0?e+t.amount:e,0),Oy=(n,e)=>n.filter(t=>t.kind===e).reduce((t,i)=>t+Vf(i),0),By=(n,e,t,i,r)=>{let s=(l,h,m,u)=>`<rect class="tr-box" x="${l}" y="${h}" width="170" height="70"/><text class="tr-label" x="${l+12}" y="${h+22}">${m}</text><text class="tr-fig" x="${l+12}" y="${h+54}">${_i(u)}</text>`,o=(l,h,m,u)=>`<line class="tr-line" x1="${l}" y1="${h}" x2="${m}" y2="${u}" marker-end="url(#tr-arrow)"/>`,a='<svg class="tr-flow" viewBox="0 0 830 180" role="img" aria-label="Where the plaza budget goes"><defs><marker id="tr-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path class="tr-head" d="M0 0L10 5L0 10z"/></marker></defs>'+s(0,55,"Budget",n)+s(220,55,"Escrow",e)+s(440,10,"Spend",t)+s(440,100,"Payment",i)+s(660,55,"Wallets",r)+o(170,90,220,90)+o(390,90,440,45)+o(390,90,440,135)+o(610,135,660,90)+"</svg>",c=v("div.tr-flow-wrap");return c.innerHTML=a,c};function Gf(n,e){let t=v("div.stack"),i=!0,r,s="",o=null,a=()=>{let x=Zt({type:"number",min:1,step:1,required:!0}),f=()=>{},d=We("Mint",()=>{let S=Number(x.value);if(!Number.isInteger(S)||S<=0){nn("Enter a whole amount above zero.","error");return}d.disabled=!0,Oe.mintBudget(S).then(()=>{nn(`Minted ${_i(S)} into the plaza budget.`),f(),u()}).catch(P=>{d.disabled=!1,nn(ku(P),"error")})});d.classList.add("primary"),f=Cn("Mint budget",v("div.stack",null,gt("Amount",x,"Whole units added to the plaza budget."),v("div.row",null,d)))},c=()=>{let x=o?.roster??[];if(x.length===0){nn("There are no agents to fine.","error");return}let f=fn(x.map(T=>({value:String(T.cid),label:T.name}))),d=Zt({type:"number",min:1,step:1,required:!0}),S=Zt({type:"text",required:!0,placeholder:"What for"}),P=()=>{},w=We("Fine",()=>{let T=Number(d.value),A=S.value.trim();if(!Number.isInteger(T)||T<=0){nn("Enter a whole amount above zero.","error");return}if(!A){nn("A fine needs a memo.","error");return}w.disabled=!0,Oe.fineAgent(Number(f.value),T,A).then(()=>{nn(`Fined ${_i(T)}.`),P(),u()}).catch(D=>{w.disabled=!1,nn(ku(D),"error")})});w.classList.add("danger"),P=Cn("Fine an agent",v("div.stack",null,gt("Agent",f),gt("Amount",d,"Taken from the agent\u2019s wallet."),gt("Memo",S,"Shown in the journal."),v("div.row",null,w)))},l=We("Fine an agent",()=>c()),h=We("Mint budget",()=>a());h.classList.add("primary"),n.append(v("div.view-head",null,v("h1",null,"Treasury"),v("div.actions",null,l,h)),t);let m=()=>{if(!o)return;let{t:x,journal:f,wallets:d,roster:S}=o,P=new Map(S.map(L=>[L.cid,L.role_label])),w=x.accounts.filter(L=>L.kind==="escrow").reduce((L,U)=>L+U.balance,0),T=Oy(f,"payment"),A=d.reduce((L,U)=>L+U.balance,0),D=[...new Set(f.map(L=>L.kind))].sort(),p=f.filter(L=>!s||L.kind===s).slice(0,50),M=[{label:"Agent",cell:L=>v("a",{href:`#/agents/${L.cid}`},L.name)},{label:"Role",cell:L=>P.get(L.cid)??"\u2014"},{label:"Balance",num:!0,cell:L=>_i(L.balance)}],y=d.slice().sort((L,U)=>U.balance-L.balance),C=fn([{value:"",label:"All kinds"},...D.map(L=>({value:L,label:L}))],s);C.addEventListener("change",()=>{s=C.value,m()});let k=p.map(L=>v("div.tr-row",null,v("span.tr-n",null,`#${L.id}`),v("span.tr-at",null,Wt(L.at)),ot(L.kind,Fy[L.kind]??"muted"),v("span.tr-memo",null,L.memo||`${L.kind}${L.task?` for ${L.task}`:""}`),v("span.tr-amt",null,_i(Vf(L)))));Ue(t,v("div.grid.tr-tallies",null,Zn("Plaza budget",_i(x.plaza_budget)),Zn("In escrow",_i(w),"held for open tasks"),Zn("Spent",_i(x.spent),"to providers and compute"),Zn("Paid to agents",_i(T),"margins for verified work"),Zn("Agents\u2019 wallets",_i(A))),zt("Flow",By(x.plaza_budget,w,x.spent,T,A)),zt("Wallets",d.length?rr(M,y):ht("user","No wallets yet."),{flush:!0}),zt("Journal",p.length?v("div.tr-ledger",null,...k):ht("clock","No entries."),{flush:!0,actions:[C]}))},u=async()=>{try{let[x,f,d,S]=await Promise.all([Oe.treasury(),Oe.treasuryJournal(),Oe.treasuryWallets(),Oe.agents()]);if(!i)return;o={t:x,journal:f.slice().sort((P,w)=>w.at-P.at),wallets:d,roster:S},m()}catch(x){i&&Ue(t,ht("alert","Treasury unreachable",ku(x)))}},_=bt("treasury",()=>{r!==void 0&&clearTimeout(r),r=setTimeout(()=>{r=void 0,u()},400)});return u(),()=>{i=!1,_(),r!==void 0&&clearTimeout(r)}}var Ol=[{key:"overview",label:"Overview",icon:"overview",section:"Plaza",mount:Nf},{key:"agents",label:"Agents",icon:"agents",section:"Plaza",mount:Zu},{key:"tasks",label:"Tasks",icon:"tasks",section:"Plaza",mount:Hf},{key:"floor",label:"Floor",icon:"floor",section:"Plaza",mount:yf},{key:"conversations",label:"Conversations",icon:"conversations",section:"Social",mount:ju},{key:"board",label:"Board",icon:"board",section:"Social",mount:Ju},{key:"council",label:"Council",icon:"council",section:"Social",mount:th},{key:"treasury",label:"Treasury",icon:"treasury",section:"Social",mount:Gf},{key:"models",label:"Models",icon:"models",section:"Setup",mount:Df},{key:"machine",label:"Machines",icon:"machine",section:"Setup",mount:Cf}];var Wf=new Map,Xf=new Map;function zy(){let n=v("nav.nav",{"aria-label":"Screens"}),e="";Ol.forEach((r,s)=>{r.section!==e&&(e=r.section,n.appendChild(v("div.nav-section",null,e)));let o=v("span.count");Xf.set(r.key,o);let a=v("a",{href:vn(r.key)},v("span.k",null,String(s+1)),r.label,v("span.dots"),o);Wf.set(r.key,a),n.appendChild(a)});let t=v("div.sidebar-foot");zu(r=>Ue(t,v("span.row",null,ci(r?"ok":"danger"),r?"Live":"Reconnecting\u2026")));let i=v("div.brand",null,v("span.brand-mark"),"AgentPlaza",v("small",null,"the plaza register"));return v("aside.sidebar",null,i,n,t)}function Hy(n){let e=v("div.health");return ir.watch(t=>{if(!t)return Ue(e,v("span",null,ci("danger"),"apid unreachable"));Ue(e,v("span",null,ci(t.hub?"ok":"danger"),"Hub"),v("span",null,ci(t.noded?"ok":"danger"),"Node"),v("span",null,ci(t.agents.working?"info":"muted"),`${t.agents.working} working \xB7 ${t.agents.idle} idle`))}),v("header.topbar",null,n,v("span.spacer"),e)}function Vy(){let n=v("div",{hidden:!0});return ir.watch(e=>{n.hidden=!e?.dev_models,Ue(n,li("alert"),"Development models are enabled: the model policy is relaxed (mock and local models allowed)."),n.className="banner danger"}),n}function Gy(){Hu();let n=v("span.title"),e=v("main.view-host"),t=v("div.app",null,zy(),v("div.main",null,Hy(n),Vy(),e));document.body.replaceChildren(t),ut.watch(s=>{let o=s.filter(a=>a.status==="working").length;Xf.get("agents").textContent=s.length?o?`${o}/${s.length}`:String(s.length):""});let i,r="";Uu(s=>{let o=Ol.find(l=>l.key===s.screen)??Ol[0];for(let[l,h]of Wf)l===o.key?h.setAttribute("aria-current","page"):h.removeAttribute("aria-current");let a=`${o.key}/${s.params.join("/")}`;if(a===r)return;r=a,i?.(),n.textContent=o.label,document.title=`${o.label} \xB7 AgentPlaza`;let c=v("div.view");e.replaceChildren(c),i=o.mount(c,s.params)})}Gy();
/*! Bundled license information:

three/build/three.core.js:
three/build/three.module.js:
  (**
   * @license
   * Copyright 2010-2026 Three.js Authors
   * SPDX-License-Identifier: MIT
   *)
*/
