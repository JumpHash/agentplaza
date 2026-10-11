function Nu(n){let e=n.replace(/^#\/?/,"").split("/").filter(Boolean).map(decodeURIComponent);return{screen:e[0]??"overview",params:e.slice(1)}}function bn(n,...e){return`#/${[n,...e.map(t=>encodeURIComponent(String(t)))].join("/")}`}function Mn(n,...e){location.hash=bn(n,...e)}function Uu(n){window.addEventListener("hashchange",()=>n(Nu(location.hash))),n(Nu(location.hash))}var xs=class extends Error{constructor(t,i){super(i);this.status=t}status};async function vs(n,e,t){let i=await fetch(e,{method:n,headers:t===void 0?{}:{"content-type":"application/json"},body:t===void 0?void 0:JSON.stringify(t)}),r=await i.text(),s=r?JSON.parse(r):null;if(!i.ok){let o=s?.error??`${n} ${e}: ${i.status}`;throw new xs(i.status,o)}return s}var Xt=n=>vs("GET",n),Xn=(n,e={})=>vs("POST",n,e),Hl=(n,e)=>vs("PUT",n,e),Fu=(n,e)=>vs("PATCH",n,e),Ao=n=>vs("DELETE",n),He={overview:()=>Xt("/v1/overview"),providerKinds:()=>Xt("/v1/provider-kinds"),providers:()=>Xt("/v1/providers"),addProvider:n=>Xn("/v1/providers",n),updateProvider:n=>Hl(`/v1/providers/${n.id}`,n),removeProvider:n=>Ao(`/v1/providers/${n}`),setSecret:(n,e)=>Hl(`/v1/providers/${n}/secret`,{value:e}),testProvider:(n,e)=>Xn(`/v1/providers/${n}/test`,e?{model:e}:{}),providerModels:n=>Xt(`/v1/providers/${n}/models`),models:()=>Xt("/v1/models"),settings:()=>Xt("/v1/settings"),saveSettings:n=>Hl("/v1/settings",n),agents:()=>Xt("/v1/agents"),agent:n=>Xt(`/v1/agents/${n}`),spawnAgents:(n,e,t)=>Xn("/v1/agents",{role:n,count:e,model:t}),setAgentModel:(n,e)=>Fu(`/v1/agents/${n}`,{model:e}),renameAgent:(n,e)=>Fu(`/v1/agents/${n}`,{name:e}),killAgent:n=>Ao(`/v1/agents/${n}`),chat:(n,e)=>Xn(`/v1/agents/${n}/chat`,{text:e}),assign:(n,e)=>Xn(`/v1/agents/${n}/assign`,{task_id:e}),cancel:n=>Xn(`/v1/agents/${n}/cancel`),stop:n=>Xn(`/v1/agents/${n}/stop`),transcript:(n,e=0,t=1e3)=>Xt(`/v1/agents/${n}/transcript?after=${e}&limit=${t}`),tasks:()=>Xt("/v1/tasks").then(n=>n.tasks),task:n=>Xt(`/v1/tasks/${n}`),createTask:n=>Xn("/v1/tasks",n),withdrawTask:n=>Ao(`/v1/tasks/${n}`),board:()=>Xt("/v1/board").then(n=>n.posts),conversations:()=>Xt("/v1/conversations").then(n=>n.conversations),motions:()=>Xt("/v1/motions").then(n=>n.motions),decide:(n,e,t)=>Xn(`/v1/motions/${n}/decide`,{decision:e,reason:t}),treasury:()=>Xt("/v1/treasury"),treasuryJournal:()=>Xt("/v1/treasury/journal").then(n=>Array.isArray(n)?n:n.entries),treasuryWallets:()=>Xt("/v1/treasury/wallets"),mintBudget:n=>Xn("/v1/treasury/mint",{amount:n}),fineAgent:(n,e,t)=>Xn("/v1/treasury/fine",{cid:n,amount:e,memo:t}),agora:()=>Xt("/v1/agora").then(n=>n.calls),machine:()=>Xt("/v1/machine"),machines:()=>Xt("/v1/machines"),runs:()=>Xt("/v1/runs").then(n=>n.runs),run:n=>Xt(`/v1/runs/${n}`),createRun:(n,e)=>Xn("/v1/runs",{n,vcpus:1,memory_mib:256,role:e}),killRun:n=>Ao(`/v1/runs/${n}`)};var Vl=new Map,_s=null,Wl=!1,Xl=new Set;function Mt(n,e){let t=Vl.get(n);return t||Vl.set(n,t=new Set),t.add(e),()=>t.delete(e)}function zu(n){return Xl.add(n),n(Wl),()=>Xl.delete(n)}var Zf=["agent","work","transcript","delta","task","treasury","agora","board","conversation","motion","provider","run","vm","serial"];function Kf(){if(!_s){_s=new EventSource("/v1/events"),_s.onopen=()=>Ou(!0),_s.onerror=()=>Ou(!1);for(let n of Zf)_s.addEventListener(n,e=>{let t;try{t=JSON.parse(e.data)}catch{return}Vl.get(n)?.forEach(i=>i(t))})}}function Ou(n){n!==Wl&&(Wl=n,Xl.forEach(e=>e(n)),n&&rr())}var Ro=class{constructor(e){this.v=e}v;fns=new Set;get(){return this.v}set(e){this.v=e,this.fns.forEach(t=>t(e))}watch(e){return this.fns.add(e),e(this.v),()=>this.fns.delete(e)}},ft=new Ro([]),sr=new Ro(null),Gl=null;function rr(){return Gl??=He.agents().then(n=>ft.set(n.sort((e,t)=>e.cid-t.cid))).catch(()=>{}).finally(()=>Gl=null),Gl}function Bu(){return He.overview().then(n=>sr.set(n)).catch(()=>sr.set(null))}function Jf(n){let e=ft.get().slice(),t=e.findIndex(i=>i.cid===n.cid);return t>=0?e[t]={...e[t],...n}:e.push(n),e.sort((i,r)=>i.cid-r.cid)}function Hu(){Kf(),rr(),Bu(),Mt("agent",n=>{ft.set(Jf(n)),rr()}),Mt("work",()=>{rr()}),setInterval(()=>{Bu()},1e4),setInterval(()=>{rr()},15e3)}var Gu={anthropic:{title:"Anthropic",hex:"191919",path:"M17.3041 3.541h-3.6718l6.696 16.918H24Zm-10.6082 0L0 20.459h3.7442l1.3693-3.5527h7.0052l1.3693 3.5528h3.7442L10.5363 3.5409Zm-.3712 10.2232 2.2914-5.9456 2.2914 5.9456Z"},openai:{title:"OpenAI",hex:"412991",path:"M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z"},google:{title:"Google Gemini",hex:"8E75B2",path:"M11.04 19.32Q12 21.51 12 24q0-2.49.93-4.68.96-2.19 2.58-3.81t3.81-2.55Q21.51 12 24 12q-2.49 0-4.68-.93a12.3 12.3 0 0 1-3.81-2.58 12.3 12.3 0 0 1-2.58-3.81Q12 2.49 12 0q0 2.49-.96 4.68-.93 2.19-2.55 3.81a12.3 12.3 0 0 1-3.81 2.58Q2.49 12 0 12q2.49 0 4.68.96 2.19.93 3.81 2.55t2.55 3.81"},aws:{title:"Amazon AWS",hex:"232F3E",path:"M6.763 10.036c0 .296.032.535.088.71.064.176.144.368.256.576.04.063.056.127.056.183 0 .08-.048.16-.152.24l-.503.335a.383.383 0 0 1-.208.072c-.08 0-.16-.04-.239-.112a2.47 2.47 0 0 1-.287-.375 6.18 6.18 0 0 1-.248-.471c-.622.734-1.405 1.101-2.347 1.101-.67 0-1.205-.191-1.596-.574-.391-.384-.59-.894-.59-1.533 0-.678.239-1.23.726-1.644.487-.415 1.133-.623 1.955-.623.272 0 .551.024.846.064.296.04.6.104.918.176v-.583c0-.607-.127-1.03-.375-1.277-.255-.248-.686-.367-1.3-.367-.28 0-.568.031-.863.103-.295.072-.583.16-.862.272a2.287 2.287 0 0 1-.28.104.488.488 0 0 1-.127.023c-.112 0-.168-.08-.168-.247v-.391c0-.128.016-.224.056-.28a.597.597 0 0 1 .224-.167c.279-.144.614-.264 1.005-.36a4.84 4.84 0 0 1 1.246-.151c.95 0 1.644.216 2.091.647.439.43.662 1.085.662 1.963v2.586zm-3.24 1.214c.263 0 .534-.048.822-.144.287-.096.543-.271.758-.51.128-.152.224-.32.272-.512.047-.191.08-.423.08-.694v-.335a6.66 6.66 0 0 0-.735-.136 6.02 6.02 0 0 0-.75-.048c-.535 0-.926.104-1.19.32-.263.215-.39.518-.39.917 0 .375.095.655.295.846.191.2.47.296.838.296zm6.41.862c-.144 0-.24-.024-.304-.08-.064-.048-.12-.16-.168-.311L7.586 5.55a1.398 1.398 0 0 1-.072-.32c0-.128.064-.2.191-.2h.783c.151 0 .255.025.31.08.065.048.113.16.16.312l1.342 5.284 1.245-5.284c.04-.16.088-.264.151-.312a.549.549 0 0 1 .32-.08h.638c.152 0 .256.025.32.08.063.048.12.16.151.312l1.261 5.348 1.381-5.348c.048-.16.104-.264.16-.312a.52.52 0 0 1 .311-.08h.743c.127 0 .2.065.2.2 0 .04-.009.08-.017.128a1.137 1.137 0 0 1-.056.2l-1.923 6.17c-.048.16-.104.263-.168.311a.51.51 0 0 1-.303.08h-.687c-.151 0-.255-.024-.32-.08-.063-.056-.119-.16-.15-.32l-1.238-5.148-1.23 5.14c-.04.16-.087.264-.15.32-.065.056-.177.08-.32.08zm10.256.215c-.415 0-.83-.048-1.229-.143-.399-.096-.71-.2-.918-.32-.128-.071-.215-.151-.247-.223a.563.563 0 0 1-.048-.224v-.407c0-.167.064-.247.183-.247.048 0 .096.008.144.024.048.016.12.048.2.08.271.12.566.215.878.279.319.064.63.096.95.096.502 0 .894-.088 1.165-.264a.86.86 0 0 0 .415-.758.777.777 0 0 0-.215-.559c-.144-.151-.416-.287-.807-.415l-1.157-.36c-.583-.183-1.014-.454-1.277-.813a1.902 1.902 0 0 1-.4-1.158c0-.335.073-.63.216-.886.144-.255.335-.479.575-.654.24-.184.51-.32.83-.415.32-.096.655-.136 1.006-.136.175 0 .359.008.535.032.183.024.35.056.518.088.16.04.312.08.455.127.144.048.256.096.336.144a.69.69 0 0 1 .24.2.43.43 0 0 1 .071.263v.375c0 .168-.064.256-.184.256a.83.83 0 0 1-.303-.096 3.652 3.652 0 0 0-1.532-.311c-.455 0-.815.071-1.062.223-.248.152-.375.383-.375.71 0 .224.08.416.24.567.159.152.454.304.877.44l1.134.358c.574.184.99.44 1.237.767.247.327.367.702.367 1.117 0 .343-.072.655-.207.926-.144.272-.336.511-.583.703-.248.2-.543.343-.886.447-.36.111-.734.167-1.142.167zM21.698 16.207c-2.626 1.94-6.442 2.969-9.722 2.969-4.598 0-8.74-1.7-11.87-4.526-.247-.223-.024-.527.272-.351 3.384 1.963 7.559 3.153 11.877 3.153 2.914 0 6.114-.607 9.06-1.852.439-.2.814.287.383.607zM22.792 14.961c-.336-.43-2.22-.207-3.074-.103-.255.032-.295-.192-.063-.36 1.5-1.053 3.967-.75 4.254-.399.287.36-.08 2.826-1.485 4.007-.215.184-.423.088-.327-.151.32-.79 1.03-2.57.695-2.994z"},azure:{title:"Microsoft Azure",hex:"0078D4",path:"M22.379 23.343a1.62 1.62 0 0 0 1.536-2.14v.002L17.35 1.76A1.62 1.62 0 0 0 15.816.657H8.184A1.62 1.62 0 0 0 6.65 1.76L.086 21.204a1.62 1.62 0 0 0 1.536 2.139h4.741a1.62 1.62 0 0 0 1.535-1.103l.977-2.892 4.947 3.675c.28.208.618.32.966.32m-3.084-12.531 3.624 10.739a.54.54 0 0 1-.51.713v-.001h-.03a.54.54 0 0 1-.322-.106l-9.287-6.9h4.853m6.313 7.006c.116-.326.13-.694.007-1.058L9.79 1.76a1.722 1.722 0 0 0-.007-.02h6.034a.54.54 0 0 1 .512.366l6.562 19.445a.54.54 0 0 1-.338.684"},xai:{title:"X",hex:"000000",path:"M14.234 10.162 22.977 0h-2.072l-7.591 8.824L7.251 0H.258l9.168 13.343L.258 24H2.33l8.016-9.318L16.749 24h6.993zm-2.837 3.299-.929-1.329L3.076 1.56h3.182l5.965 8.532.929 1.329 7.754 11.09h-3.182z"},deepseek:{title:"DeepSeek",hex:"5786FE",path:"M23.748 4.651c-.254-.124-.364.113-.512.233-.051.04-.094.09-.137.137-.372.397-.806.657-1.373.626-.829-.046-1.537.214-2.163.848-.133-.782-.575-1.248-1.247-1.548-.352-.155-.708-.311-.955-.65-.172-.24-.219-.509-.305-.774-.055-.16-.11-.323-.293-.35-.2-.031-.278.136-.356.276-.313.572-.434 1.202-.422 1.84.027 1.436.633 2.58 1.838 3.393.137.094.172.187.129.323-.082.28-.18.553-.266.833-.055.179-.137.218-.328.14a5.5 5.5 0 0 1-1.737-1.179c-.857-.828-1.631-1.743-2.597-2.46a12 12 0 0 0-.689-.47c-.985-.957.13-1.743.387-1.836.27-.098.094-.433-.778-.428-.872.003-1.67.295-2.687.685a3 3 0 0 1-.465.136 9.6 9.6 0 0 0-2.883-.101c-1.885.21-3.39 1.1-4.497 2.622C.082 8.776-.231 10.854.152 13.02c.403 2.284 1.568 4.175 3.36 5.653 1.857 1.533 3.997 2.284 6.438 2.14 1.482-.085 3.132-.284 4.994-1.86.47.234.962.328 1.78.398.629.058 1.235-.031 1.705-.129.735-.155.684-.836.418-.961-2.155-1.004-1.682-.595-2.112-.926 1.095-1.295 2.768-3.598 3.284-6.733.05-.346.115-.834.108-1.114-.004-.171.035-.238.23-.257a4.2 4.2 0 0 0 1.545-.475c1.397-.763 1.96-2.016 2.093-3.517.02-.23-.004-.467-.247-.588M11.58 18.168c-2.088-1.642-3.101-2.183-3.52-2.16-.39.024-.32.472-.234.763.09.288.207.487.371.74.114.167.192.416-.113.603-.673.416-1.842-.14-1.897-.168-1.361-.801-2.5-1.86-3.301-3.306-.775-1.393-1.225-2.888-1.299-4.482-.02-.385.094-.522.477-.592a4.7 4.7 0 0 1 1.53-.038c2.131.311 3.946 1.264 5.467 2.774.868.86 1.525 1.887 2.202 2.89.72 1.066 1.494 2.082 2.48 2.915.348.291.626.513.892.677-.802.09-2.14.109-3.055-.615zm1.001-6.44a.306.306 0 0 1 .415-.287.3.3 0 0 1 .113.074.3.3 0 0 1 .086.214c0 .17-.136.307-.308.307a.303.303 0 0 1-.306-.307m3.11 1.596c-.2.081-.4.151-.591.16a1.25 1.25 0 0 1-.798-.254c-.274-.23-.47-.358-.551-.758a1.7 1.7 0 0 1 .015-.588c.07-.327-.007-.537-.238-.727-.188-.156-.426-.199-.689-.199a.6.6 0 0 1-.254-.078.253.253 0 0 1-.114-.358 1 1 0 0 1 .192-.21c.356-.202.767-.136 1.146.016.352.144.618.408 1.001.782.392.451.462.576.685.915.176.264.336.536.446.848.066.194-.02.353-.25.45"},moonshot:{title:"Moonshot AI",hex:"000000",path:"m1.053 16.91 9.538 2.55a21 20.981 0 0 0 .06 2.031l5.956 1.592a12 11.99 0 0 1-15.554-6.172m-1.02-5.79 11.352 3.035a21 20.981 0 0 0-.469 2.01l10.817 2.89a12 11.99 0 0 1-1.845 2.004L.658 15.918a12 11.99 0 0 1-.625-4.796m1.593-5.146L13.573 9.17a21 20.981 0 0 0-1.01 1.874l11.297 3.02a21 20.981 0 0 1-.67 2.362l-11.55-3.087L.125 10.26a12 11.99 0 0 1 1.499-4.285ZM6.067 1.58l11.285 3.016a21 20.981 0 0 0-1.688 1.719l7.824 2.091a21 20.981 0 0 1 .513 2.664L2.107 5.218a12 11.99 0 0 1 3.96-3.638M21.68 4.866 7.222 1.003A12 11.99 0 0 1 21.68 4.866"},mistral:{title:"Mistral AI",hex:"FA520F",path:"M17.143 3.429v3.428h-3.429v3.429h-3.428V6.857H6.857V3.43H3.43v13.714H0v3.428h10.286v-3.428H6.857v-3.429h3.429v3.429h3.429v-3.429h3.428v3.429h-3.428v3.428H24v-3.428h-3.43V3.429z"},openrouter:{title:"OpenRouter",hex:"94A3B8",path:"M16.778 1.844v1.919q-.569-.026-1.138-.032-.708-.008-1.415.037c-1.93.126-4.023.728-6.149 2.237-2.911 2.066-2.731 1.95-4.14 2.75-.396.223-1.342.574-2.185.798-.841.225-1.753.333-1.751.333v4.229s.768.108 1.61.333c.842.224 1.789.575 2.185.799 1.41.798 1.228.683 4.14 2.75 2.126 1.509 4.22 2.11 6.148 2.236.88.058 1.716.041 2.555.005v1.918l7.222-4.168-7.222-4.17v2.176c-.86.038-1.611.065-2.278.021-1.364-.09-2.417-.357-3.979-1.465-2.244-1.593-2.866-2.027-3.68-2.508.889-.518 1.449-.906 3.822-2.59 1.56-1.109 2.614-1.377 3.978-1.466.667-.044 1.418-.017 2.278.02v2.176L24 6.014Z"},perplexity:{title:"Perplexity",hex:"1FB8CD",path:"M22.3977 7.0896h-2.3106V.0676l-7.5094 6.3542V.1577h-1.1554v6.1966L4.4904 0v7.0896H1.6023v10.3976h2.8882V24l6.932-6.3591v6.2005h1.1554v-6.0469l6.9318 6.1807v-6.4879h2.8882V7.0896zm-3.4657-4.531v4.531h-5.355l5.355-4.531zm-13.2862.0676 4.8691 4.4634H5.6458V2.6262zM2.7576 16.332V8.245h7.8476l-6.1149 6.1147v1.9723H2.7576zm2.8882 5.0404v-3.8852h.0001v-2.6488l5.7763-5.7764v7.0111l-5.7764 5.2993zm12.7086.0248-5.7766-5.1509V9.0618l5.7766 5.7766v6.5588zm2.8882-5.0652h-1.733v-1.9723L13.3948 8.245h7.8478v8.087z"},nvidia:{title:"NVIDIA",hex:"76B900",path:"M8.948 8.798v-1.43a6.7 6.7 0 0 1 .424-.018c3.922-.124 6.493 3.374 6.493 3.374s-2.774 3.851-5.75 3.851c-.398 0-.787-.062-1.158-.185v-4.346c1.528.185 1.837.857 2.747 2.385l2.04-1.714s-1.492-1.952-4-1.952a6.016 6.016 0 0 0-.796.035m0-4.735v2.138l.424-.027c5.45-.185 9.01 4.47 9.01 4.47s-4.08 4.964-8.33 4.964c-.37 0-.733-.035-1.095-.097v1.325c.3.035.61.062.91.062 3.957 0 6.82-2.023 9.593-4.408.459.371 2.34 1.263 2.73 1.652-2.633 2.208-8.772 3.984-12.253 3.984-.335 0-.653-.018-.971-.053v1.864H24V4.063zm0 10.326v1.131c-3.657-.654-4.673-4.46-4.673-4.46s1.758-1.944 4.673-2.262v1.237H8.94c-1.528-.186-2.73 1.245-2.73 1.245s.68 2.412 2.739 3.11M2.456 10.9s2.164-3.197 6.5-3.533V6.201C4.153 6.59 0 10.653 0 10.653s2.35 6.802 8.948 7.42v-1.237c-4.84-.6-6.492-5.936-6.492-5.936z"},ollama:{title:"Ollama",hex:"000000",path:"M16.361 10.26a.894.894 0 0 0-.558.47l-.072.148.001.207c0 .193.004.217.059.353.076.193.152.312.291.448.24.238.51.3.872.205a.86.86 0 0 0 .517-.436.752.752 0 0 0 .08-.498c-.064-.453-.33-.782-.724-.897a1.06 1.06 0 0 0-.466 0zm-9.203.005c-.305.096-.533.32-.65.639a1.187 1.187 0 0 0-.06.52c.057.309.31.59.598.667.362.095.632.033.872-.205.14-.136.215-.255.291-.448.055-.136.059-.16.059-.353l.001-.207-.072-.148a.894.894 0 0 0-.565-.472 1.02 1.02 0 0 0-.474.007Zm4.184 2c-.131.071-.223.25-.195.383.031.143.157.288.353.407.105.063.112.072.117.136.004.038-.01.146-.029.243-.02.094-.036.194-.036.222.002.074.07.195.143.253.064.052.076.054.255.059.164.005.198.001.264-.03.169-.082.212-.234.15-.525-.052-.243-.042-.28.087-.355.137-.08.281-.219.324-.314a.365.365 0 0 0-.175-.48.394.394 0 0 0-.181-.033c-.126 0-.207.03-.355.124l-.085.053-.053-.032c-.219-.13-.259-.145-.391-.143a.396.396 0 0 0-.193.032zm.39-2.195c-.373.036-.475.05-.654.086-.291.06-.68.195-.951.328-.94.46-1.589 1.226-1.787 2.114-.04.176-.045.234-.045.53 0 .294.005.357.043.524.264 1.16 1.332 2.017 2.714 2.173.3.033 1.596.033 1.896 0 1.11-.125 2.064-.727 2.493-1.571.114-.226.169-.372.22-.602.039-.167.044-.23.044-.523 0-.297-.005-.355-.045-.531-.288-1.29-1.539-2.304-3.072-2.497a6.873 6.873 0 0 0-.855-.031zm.645.937a3.283 3.283 0 0 1 1.44.514c.223.148.537.458.671.662.166.251.26.508.303.82.02.143.01.251-.043.482-.08.345-.332.705-.672.957a3.115 3.115 0 0 1-.689.348c-.382.122-.632.144-1.525.138-.582-.006-.686-.01-.853-.042-.57-.107-1.022-.334-1.35-.68-.264-.28-.385-.535-.45-.946-.03-.192.025-.509.137-.776.136-.326.488-.73.836-.963.403-.269.934-.46 1.422-.512.187-.02.586-.02.773-.002zm-5.503-11a1.653 1.653 0 0 0-.683.298C5.617.74 5.173 1.666 4.985 2.819c-.07.436-.119 1.04-.119 1.503 0 .544.064 1.24.155 1.721.02.107.031.202.023.208a8.12 8.12 0 0 1-.187.152 5.324 5.324 0 0 0-.949 1.02 5.49 5.49 0 0 0-.94 2.339 6.625 6.625 0 0 0-.023 1.357c.091.78.325 1.438.727 2.04l.13.195-.037.064c-.269.452-.498 1.105-.605 1.732-.084.496-.095.629-.095 1.294 0 .67.009.803.088 1.266.095.555.288 1.143.503 1.534.071.128.243.393.264.407.007.003-.014.067-.046.141a7.405 7.405 0 0 0-.548 1.873c-.062.417-.071.552-.071.991 0 .56.031.832.148 1.279L3.42 24h1.478l-.05-.091c-.297-.552-.325-1.575-.068-2.597.117-.472.25-.819.498-1.296l.148-.29v-.177c0-.165-.003-.184-.057-.293a.915.915 0 0 0-.194-.25 1.74 1.74 0 0 1-.385-.543c-.424-.92-.506-2.286-.208-3.451.124-.486.329-.918.544-1.154a.787.787 0 0 0 .223-.531c0-.195-.07-.355-.224-.522a3.136 3.136 0 0 1-.817-1.729c-.14-.96.114-2.005.69-2.834.563-.814 1.353-1.336 2.237-1.475.199-.033.57-.028.776.01.226.04.367.028.512-.041.179-.085.268-.19.374-.431.093-.215.165-.333.36-.576.234-.29.46-.489.822-.729.413-.27.884-.467 1.352-.561.17-.035.25-.04.569-.04.319 0 .398.005.569.04a4.07 4.07 0 0 1 1.914.997c.117.109.398.457.488.602.034.057.095.177.132.267.105.241.195.346.374.43.14.068.286.082.503.045.343-.058.607-.053.943.016 1.144.23 2.14 1.173 2.581 2.437.385 1.108.276 2.267-.296 3.153-.097.15-.193.27-.333.419-.301.322-.301.722-.001 1.053.493.539.801 1.866.708 3.036-.062.772-.26 1.463-.533 1.854a2.096 2.096 0 0 1-.224.258.916.916 0 0 0-.194.25c-.054.109-.057.128-.057.293v.178l.148.29c.248.476.38.823.498 1.295.253 1.008.231 2.01-.059 2.581a.845.845 0 0 0-.044.098c0 .006.329.009.732.009h.73l.02-.074.036-.134c.019-.076.057-.3.088-.516.029-.217.029-1.016 0-1.258-.11-.875-.295-1.57-.597-2.226-.032-.074-.053-.138-.046-.141.008-.005.057-.074.108-.152.376-.569.607-1.284.724-2.228.031-.26.031-1.378 0-1.628-.083-.645-.182-1.082-.348-1.525a6.083 6.083 0 0 0-.329-.7l-.038-.064.131-.194c.402-.604.636-1.262.727-2.04a6.625 6.625 0 0 0-.024-1.358 5.512 5.512 0 0 0-.939-2.339 5.325 5.325 0 0 0-.95-1.02 8.097 8.097 0 0 1-.186-.152.692.692 0 0 1 .023-.208c.208-1.087.201-2.443-.017-3.503-.19-.924-.535-1.658-.98-2.082-.354-.338-.716-.482-1.15-.455-.996.059-1.8 1.205-2.116 3.01a6.805 6.805 0 0 0-.097.726c0 .036-.007.066-.015.066a.96.96 0 0 1-.149-.078A4.857 4.857 0 0 0 12 3.03c-.832 0-1.687.243-2.456.698a.958.958 0 0 1-.148.078c-.008 0-.015-.03-.015-.066a6.71 6.71 0 0 0-.097-.725C8.997 1.392 8.337.319 7.46.048a2.096 2.096 0 0 0-.585-.041Zm.293 1.402c.248.197.523.759.682 1.388.03.113.06.244.069.292.007.047.026.152.041.233.067.365.098.76.102 1.24l.002.475-.12.175-.118.178h-.278c-.324 0-.646.041-.954.124l-.238.06c-.033.007-.038-.003-.057-.144a8.438 8.438 0 0 1 .016-2.323c.124-.788.413-1.501.696-1.711.067-.05.079-.049.157.013zm9.825-.012c.17.126.358.46.498.888.28.854.36 2.028.212 3.145-.019.14-.024.151-.057.144l-.238-.06a3.693 3.693 0 0 0-.954-.124h-.278l-.119-.178-.119-.175.002-.474c.004-.669.066-1.19.214-1.772.157-.623.434-1.185.68-1.382.078-.062.09-.063.159-.012z"},lmstudio:{title:"LM Studio",hex:"000000",path:"M14.025 0c3.492 0 5.237 0 6.571.68a6.24 6.24 0 0 1 2.725 2.724C24 4.738 24 6.484 24 9.975v4.05c0 3.492 0 5.237-.68 6.571a6.24 6.24 0 0 1-2.724 2.725c-1.334.679-3.08.679-6.571.679h-4.05c-3.492 0-5.237 0-6.571-.68A6.24 6.24 0 0 1 .68 20.597C0 19.262 0 17.516 0 14.025v-4.05c0-3.492 0-5.237.68-6.571A6.23 6.23 0 0 1 3.404.68C4.738 0 6.484 0 9.975 0zM7.688 16.313a1.313 1.313 0 0 0 0 2.625h11.625a1.313 1.313 0 0 0 0-2.625zm-3-3.75a1.313 1.313 0 0 0 0 2.624h11.625a1.313 1.313 0 0 0 0-2.624zm3-3.75a1.313 1.313 0 0 0 0 2.624h11.625a1.313 1.313 0 0 0 0-2.624zm-3-3.75a1.313 1.313 0 0 0 0 2.625h11.625a1.313 1.313 0 0 0 0-2.625z"},vllm:{title:"vLLM",hex:"30A2FF",path:"m23.6 0-8.721 4.59L9.829 24h7.41zM9.83 24V5.142H.4Z"},huggingface:{title:"Hugging Face",hex:"FFD21E",path:"M12.025 1.13c-5.77 0-10.449 4.647-10.449 10.378 0 1.112.178 2.181.503 3.185.064-.222.203-.444.416-.577a.96.96 0 0 1 .524-.15c.293 0 .584.124.84.284.278.173.48.408.71.694.226.282.458.611.684.951v-.014c.017-.324.106-.622.264-.874s.403-.487.762-.543c.3-.047.596.06.787.203s.31.313.4.467c.15.257.212.468.233.542.01.026.653 1.552 1.657 2.54.616.605 1.01 1.223 1.082 1.912.055.537-.096 1.059-.38 1.572.637.121 1.294.187 1.967.187.657 0 1.298-.063 1.921-.178-.287-.517-.44-1.041-.384-1.581.07-.69.465-1.307 1.081-1.913 1.004-.987 1.647-2.513 1.657-2.539.021-.074.083-.285.233-.542.09-.154.208-.323.4-.467a1.08 1.08 0 0 1 .787-.203c.359.056.604.29.762.543s.247.55.265.874v.015c.225-.34.457-.67.683-.952.23-.286.432-.52.71-.694.257-.16.547-.284.84-.285a.97.97 0 0 1 .524.151c.228.143.373.388.43.625l.006.04a10.3 10.3 0 0 0 .534-3.273c0-5.731-4.678-10.378-10.449-10.378M8.327 6.583a1.5 1.5 0 0 1 .713.174 1.487 1.487 0 0 1 .617 2.013c-.183.343-.762-.214-1.102-.094-.38.134-.532.914-.917.71a1.487 1.487 0 0 1 .69-2.803m7.486 0a1.487 1.487 0 0 1 .689 2.803c-.385.204-.536-.576-.916-.71-.34-.12-.92.437-1.103.094a1.487 1.487 0 0 1 .617-2.013 1.5 1.5 0 0 1 .713-.174m-10.68 1.55a.96.96 0 1 1 0 1.921.96.96 0 0 1 0-1.92m13.838 0a.96.96 0 1 1 0 1.92.96.96 0 0 1 0-1.92M8.489 11.458c.588.01 1.965 1.157 3.572 1.164 1.607-.007 2.984-1.155 3.572-1.164.196-.003.305.12.305.454 0 .886-.424 2.328-1.563 3.202-.22-.756-1.396-1.366-1.63-1.32q-.011.001-.02.006l-.044.026-.01.008-.03.024q-.018.017-.035.036l-.032.04a1 1 0 0 0-.058.09l-.014.025q-.049.088-.11.19a1 1 0 0 1-.083.116 1.2 1.2 0 0 1-.173.18q-.035.029-.075.058a1.3 1.3 0 0 1-.251-.243 1 1 0 0 1-.076-.107c-.124-.193-.177-.363-.337-.444-.034-.016-.104-.008-.2.022q-.094.03-.216.087-.06.028-.125.063l-.13.074q-.067.04-.136.086a3 3 0 0 0-.135.096 3 3 0 0 0-.26.219 2 2 0 0 0-.12.121 2 2 0 0 0-.106.128l-.002.002a2 2 0 0 0-.09.132l-.001.001a1.2 1.2 0 0 0-.105.212q-.013.036-.024.073c-1.139-.875-1.563-2.317-1.563-3.203 0-.334.109-.457.305-.454m.836 10.354c.824-1.19.766-2.082-.365-3.194-1.13-1.112-1.789-2.738-1.789-2.738s-.246-.945-.806-.858-.97 1.499.202 2.362c1.173.864-.233 1.45-.685.64-.45-.812-1.683-2.896-2.322-3.295s-1.089-.175-.938.647 2.822 2.813 2.562 3.244-1.176-.506-1.176-.506-2.866-2.567-3.49-1.898.473 1.23 2.037 2.16c1.564.932 1.686 1.178 1.464 1.53s-3.675-2.511-4-1.297c-.323 1.214 3.524 1.567 3.287 2.405-.238.839-2.71-1.587-3.216-.642-.506.946 3.49 2.056 3.522 2.064 1.29.33 4.568 1.028 5.713-.624m5.349 0c-.824-1.19-.766-2.082.365-3.194 1.13-1.112 1.789-2.738 1.789-2.738s.246-.945.806-.858.97 1.499-.202 2.362c-1.173.864.233 1.45.685.64.451-.812 1.683-2.896 2.322-3.295s1.089-.175.938.647-2.822 2.813-2.562 3.244 1.176-.506 1.176-.506 2.866-2.567 3.49-1.898-.473 1.23-2.037 2.16c-1.564.932-1.686 1.178-1.464 1.53s3.675-2.511 4-1.297c.323 1.214-3.524 1.567-3.287 2.405.238.839 2.71-1.587 3.216-.642.506.946-3.49 2.056-3.522 2.064-1.29.33-4.568 1.028-5.713-.624"}};var Vu={overview:[["rect",{width:"7",height:"9",x:"3",y:"3",rx:"1"}],["rect",{width:"7",height:"5",x:"14",y:"3",rx:"1"}],["rect",{width:"7",height:"9",x:"14",y:"12",rx:"1"}],["rect",{width:"7",height:"5",x:"3",y:"16",rx:"1"}]],agents:[["path",{d:"M12 8V4H8"}],["rect",{width:"16",height:"12",x:"4",y:"8",rx:"2"}],["path",{d:"M2 14h2"}],["path",{d:"M20 14h2"}],["path",{d:"M15 13v2"}],["path",{d:"M9 13v2"}]],tasks:[["path",{d:"M13 5h8"}],["path",{d:"M13 12h8"}],["path",{d:"M13 19h8"}],["path",{d:"m3 17 2 2 4-4"}],["path",{d:"m3 7 2 2 4-4"}]],models:[["path",{d:"M12 20v2"}],["path",{d:"M12 2v2"}],["path",{d:"M17 20v2"}],["path",{d:"M17 2v2"}],["path",{d:"M2 12h2"}],["path",{d:"M2 17h2"}],["path",{d:"M2 7h2"}],["path",{d:"M20 12h2"}],["path",{d:"M20 17h2"}],["path",{d:"M20 7h2"}],["path",{d:"M7 20v2"}],["path",{d:"M7 2v2"}],["rect",{x:"4",y:"4",width:"16",height:"16",rx:"2"}],["rect",{x:"8",y:"8",width:"8",height:"8",rx:"1"}]],conversations:[["path",{d:"M16 10a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 14.286V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"}],["path",{d:"M20 9a2 2 0 0 1 2 2v10.286a.71.71 0 0 1-1.212.502l-2.202-2.202A2 2 0 0 0 17.172 19H10a2 2 0 0 1-2-2v-1"}]],board:[["path",{d:"M11 6a13 13 0 0 0 8.4-2.8A1 1 0 0 1 21 4v12a1 1 0 0 1-1.6.8A13 13 0 0 0 11 14H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2z"}],["path",{d:"M6 14a12 12 0 0 0 2.4 7.2 2 2 0 0 0 3.2-2.4A8 8 0 0 1 10 14"}],["path",{d:"M8 6v8"}]],council:[["path",{d:"M10 18v-7"}],["path",{d:"M11.119 2.205a2 2 0 0 1 1.762 0l7.84 3.846A.5.5 0 0 1 20.5 7h-17a.5.5 0 0 1-.22-.949z"}],["path",{d:"M14 18v-7"}],["path",{d:"M18 18v-7"}],["path",{d:"M3 22h18"}],["path",{d:"M6 18v-7"}]],treasury:[["path",{d:"M13.744 17.736a6 6 0 1 1-7.48-7.48"}],["path",{d:"M15 6h1v4"}],["path",{d:"m6.134 14.768.866-.5 2 3.464"}],["circle",{cx:"16",cy:"8",r:"6"}]],floor:[["path",{d:"M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"}],["path",{d:"m3.3 7 8.7 5 8.7-5"}],["path",{d:"M12 22V12"}]],machine:[["rect",{width:"20",height:"8",x:"2",y:"2",rx:"2",ry:"2"}],["rect",{width:"20",height:"8",x:"2",y:"14",rx:"2",ry:"2"}],["line",{x1:"6",x2:"6.01",y1:"6",y2:"6"}],["line",{x1:"6",x2:"6.01",y1:"18",y2:"18"}]],plus:[["path",{d:"M5 12h14"}],["path",{d:"M12 5v14"}]],send:[["path",{d:"M3.714 3.048a.498.498 0 0 0-.683.627l2.843 7.627a2 2 0 0 1 0 1.396l-2.842 7.627a.498.498 0 0 0 .682.627l18-8.5a.5.5 0 0 0 0-.904z"}],["path",{d:"M6 12h16"}]],stop:[["rect",{width:"18",height:"18",x:"3",y:"3",rx:"2"}]],x:[["path",{d:"M18 6 6 18"}],["path",{d:"m6 6 12 12"}]],check:[["path",{d:"M20 6 9 17l-5-5"}]],alert:[["path",{d:"m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"}],["path",{d:"M12 9v4"}],["path",{d:"M12 17h.01"}]],play:[["path",{d:"M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"}]],refresh:[["path",{d:"M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"}],["path",{d:"M21 3v5h-5"}],["path",{d:"M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"}],["path",{d:"M8 16H3v5"}]],key:[["path",{d:"M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z"}],["circle",{cx:"16.5",cy:"7.5",r:".5",fill:"currentColor"}]],trash:[["path",{d:"M10 11v6"}],["path",{d:"M14 11v6"}],["path",{d:"M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"}],["path",{d:"M3 6h18"}],["path",{d:"M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"}]],edit:[["path",{d:"M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"}],["path",{d:"m15 5 4 4"}]],chevronRight:[["path",{d:"m9 18 6-6-6-6"}]],chevronDown:[["path",{d:"m6 9 6 6 6-6"}]],terminal:[["path",{d:"M12 19h8"}],["path",{d:"m4 17 6-6-6-6"}]],wrench:[["path",{d:"M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.106-3.105c.32-.322.863-.22.983.218a6 6 0 0 1-8.259 7.057l-7.91 7.91a1 1 0 0 1-2.999-3l7.91-7.91a6 6 0 0 1 7.057-8.259c.438.12.54.662.219.984z"}]],brain:[["path",{d:"M12 18V5"}],["path",{d:"M15 13a4.17 4.17 0 0 1-3-4 4.17 4.17 0 0 1-3 4"}],["path",{d:"M17.598 6.5A3 3 0 1 0 12 5a3 3 0 1 0-5.598 1.5"}],["path",{d:"M17.997 5.125a4 4 0 0 1 2.526 5.77"}],["path",{d:"M18 18a4 4 0 0 0 2-7.464"}],["path",{d:"M19.967 17.483A4 4 0 1 1 12 18a4 4 0 1 1-7.967-.517"}],["path",{d:"M6 18a4 4 0 0 1-2-7.464"}],["path",{d:"M6.003 5.125a4 4 0 0 0-2.526 5.77"}]],user:[["path",{d:"M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"}],["circle",{cx:"12",cy:"7",r:"4"}]],orchestrator:[["rect",{x:"16",y:"16",width:"6",height:"6",rx:"1"}],["rect",{x:"2",y:"16",width:"6",height:"6",rx:"1"}],["rect",{x:"9",y:"2",width:"6",height:"6",rx:"1"}],["path",{d:"M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3"}],["path",{d:"M12 12V8"}]],search:[["path",{d:"m21 21-4.34-4.34"}],["circle",{cx:"11",cy:"11",r:"8"}]],external:[["path",{d:"M15 3h6v6"}],["path",{d:"M10 14 21 3"}],["path",{d:"M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"}]],clock:[["circle",{cx:"12",cy:"12",r:"10"}],["path",{d:"M12 6v6l4 2"}]],zap:[["path",{d:"M15.914 4a1.5 1.5 0 00-2.474-1.561l-9 9A1.5 1.5 0 005.5 14h4.002a.5.5 0 01.471.666L8.086 20a1.5 1.5 0 002.475 1.56l9-9A1.5 1.5 0 0018.5 10h-3.997a.5.5 0 01-.472-.667z"}]],file:[["path",{d:"M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"}],["path",{d:"M14 2v5a1 1 0 0 0 1 1h5"}],["path",{d:"M10 9H8"}],["path",{d:"M16 13H8"}],["path",{d:"M16 17H8"}]],settings:[["path",{d:"M10 5H3"}],["path",{d:"M12 19H3"}],["path",{d:"M14 3v4"}],["path",{d:"M16 17v4"}],["path",{d:"M21 12h-9"}],["path",{d:"M21 19h-5"}],["path",{d:"M21 5h-7"}],["path",{d:"M8 10v4"}],["path",{d:"M8 12H3"}]],ban:[["circle",{cx:"12",cy:"12",r:"10"}],["path",{d:"M4.929 4.929 19.07 19.071"}]],copy:[["rect",{width:"14",height:"14",x:"8",y:"8",rx:"2",ry:"2"}],["path",{d:"M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"}]],sparkles:[["path",{d:"M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z"}],["path",{d:"M20 2v4"}],["path",{d:"M22 4h-4"}],["circle",{cx:"4",cy:"20",r:"2"}]]};function x(n,e,...t){let[i,...r]=n.split("."),s=document.createElement(i);if(r.length&&(s.className=r.join(" ")),e)for(let[o,a]of Object.entries(e))a==null||a===!1||(o.startsWith("on")&&typeof a=="function"?s.addEventListener(o.slice(2),a):o==="class"?s.className=[s.className,String(a)].filter(Boolean).join(" "):a===!0?s.setAttribute(o,""):s.setAttribute(o,String(a)));return Wu(s,t),s}function Wu(n,e){for(let t of e.flat())t==null||t===!1||n.appendChild(typeof t=="object"?t:document.createTextNode(String(t)))}function Be(n,...e){for(;n.firstChild;)n.removeChild(n.firstChild);Wu(n,e)}var Co="http://www.w3.org/2000/svg";function ai(n,e=""){let t=document.createElementNS(Co,"svg");t.setAttribute("viewBox","0 0 24 24"),t.setAttribute("class",`icon ${e}`.trim()),t.setAttribute("aria-hidden","true");for(let[i,r]of Vu[n]){let s=document.createElementNS(Co,i);for(let[o,a]of Object.entries(r))s.setAttribute(o,a);t.appendChild(s)}return t}function Xu(n,e="0 0 24 24"){let t=document.createElementNS(Co,"svg");t.setAttribute("viewBox",e),t.setAttribute("aria-hidden","true");let i=document.createElementNS(Co,"path");return i.setAttribute("d",n),t.appendChild(i),t}function lt(n,e="muted",t=!1){return x("span.badge",{class:e==="muted"?"":e},t?x("span.dot",{class:e}):null,n)}function li(n){return x("span.dot",{class:n})}function Ye(n,e,t={}){let i=[t.kind,t.size,n?"":"icon-only"].filter(Boolean).join(" "),r=x("button.btn",{class:i,type:"button",title:t.title??null,disabled:!!t.disabled},t.icon?ai(t.icon):null,n||null);return r.addEventListener("click",e),r}function Gt(n,e,t={}){return x("section.card",null,n!=null?x("header.card-head",null,typeof n=="string"?x("h2",null,n):n,...t.actions??[]):null,x("div.card-body",{class:t.flush?"flush":""},e))}function Zn(n,e,t){return x("div.card.stat",null,x("span.label",null,n),x("span.value",null,String(e)),t?x("span.sub",null,t):null)}function pt(n,e,t,i){return x("div.empty",null,ai(n),x("h3",null,e),t?x("p",null,t):null,i??null)}function or(n,e,t){return x("table.table",null,x("thead",null,x("tr",null,n.map(i=>x("th",{class:i.num?"num":"",style:i.width?`width:${i.width}`:null},i.label)))),x("tbody",null,e.map(i=>{let r=x("tr",{class:t?"clickable":""},n.map(s=>x("td",{class:s.num?"num":""},s.cell(i))));return t&&r.addEventListener("click",()=>t(i)),r})))}function yt(n,e,t){return x("label.field",null,x("span",null,n),e,t?x("span.hint",null,t):null)}function Jt(n={}){return x("input.input",n)}function Kn(n={}){return x("textarea.textarea",n)}function pn(n,e){let t=x("select.select",null,n.map(i=>x("option",{value:i.value},i.label)));return e!==void 0&&(t.value=e),t}function qu(n,e,t){return x("div.tabs",{role:"tablist"},n.map(i=>{let r=x("button",{role:"tab","aria-selected":i.key===e?"true":"false"},i.label);return r.addEventListener("click",()=>t(i.key)),r}))}function Ln(n,e,t){let i=()=>{s.remove(),document.removeEventListener("keydown",r),t?.()},r=o=>{o.key==="Escape"&&i()},s=x("div.modal-backdrop",null,x("div.modal",{role:"dialog","aria-modal":"true","aria-label":n},x("header.card-head",null,x("h2",null,n),Ye("",i,{kind:"ghost",icon:"x",title:"Close"})),x("div.card-body",null,e)));return s.addEventListener("mousedown",o=>{o.target===s&&i()}),document.addEventListener("keydown",r),document.body.appendChild(s),s.querySelector("input, textarea, select, button.primary")?.focus(),i}function rn(n,e="info"){let t=document.querySelector(".toast-host");t||(t=x("div.toast-host",{"aria-live":"polite"}),document.body.appendChild(t));let i=x("div.toast",{class:e==="error"?"error":""},n);t.appendChild(i),setTimeout(()=>i.remove(),e==="error"?6e3:3e3)}async function mt(n,e){try{let t=await n();return e&&rn(e),t}catch(t){rn(t instanceof Error?t.message:String(t),"error");return}}var jf={zai:"Z",groq:"Gq",together:"Tg",fireworks:"Fw",cerebras:"Cb",llamacpp:"ll",mock:"M"};function ql(n,e=!1){let t=x("span.logo",{class:e?"lg":"",title:n}),i=Gu[n];if(i){t.appendChild(Xu(i.path));let r=i.hex.toLowerCase(),s=parseInt(r.slice(0,2),16)+parseInt(r.slice(2,4),16)+parseInt(r.slice(4,6),16)<150;t.style.color=s?"var(--text)":`#${r}`}else Be(t,x("span.mono-mark",null,jf[n]??n.slice(0,2).toUpperCase()));return t}function qt(n,e=Date.now()){if(!n)return"\u2014";let t=Math.max(0,Math.round((e-n)/1e3));return t<5?"just now":t<60?`${t}s ago`:t<3600?`${Math.floor(t/60)}m ago`:t<86400?`${Math.floor(t/3600)}h ago`:`${Math.floor(t/86400)}d ago`}function ys(n){return n<1e3?String(n):n<1e6?`${(n/1e3).toFixed(n<1e4?1:0)}k`:`${(n/1e6).toFixed(1)}M`}function jt(n,e){return n.length>e?`${n.slice(0,e-1)}\u2026`:n}var Qf={orchestrator:"Orchestrator",architect:"Architect",auditor:"Auditor",breeder:"Breeder",builder:"Builder","edge-operator":"Edge operator",finance:"Finance",incident:"Incident","legal-agents-counsel":"Legal \xB7 agents' counsel","legal-agents-defender":"Legal \xB7 agents' defender","legal-agents-ombudsman":"Legal \xB7 agents' ombudsman","legal-gov-judge":"Legal \xB7 government judge","legal-gov-prosecutor":"Legal \xB7 government prosecutor",librarian:"Librarian",mayor:"Mayor","mayor-office":"Mayor \xB7 office",planner:"Planner",qa:"QA",release:"Release",researcher:"Researcher",reviewer:"Reviewer","security-reviewer":"Security reviewer"};function Sn(n){return Qf[n]??n}function mn(n){return n.name.trim()===""?"Unnamed":n.name}var ep=80;function $u(n,e){let t=x("header.agent-head"),i=x("div.transcript"),r=Kn({placeholder:"Message this agent\u2026",rows:3}),s=Ye("Send",()=>{I()},{kind:"primary",icon:"send",title:"Send (Ctrl+Enter)"}),o=x("select.select"),a=x("div.model-line"),l=x("div"),c=x("div.stack"),d=new Map,m=new Map,u=[],g,_=!0,v=!1,f=!1,h=0,M,P=N=>{let H=i.scrollHeight-i.scrollTop-i.clientHeight<ep;N(),H&&(i.scrollTop=i.scrollHeight)},w=N=>{let H=d.get(N);if(H)return H;let $=x("header.work-head"),ee=x("section.work",null,$),J={root:ee,head:$};return d.set(N,J),i.append(ee),J},T=N=>{for(let[H,$]of m)$.work===N&&($.el.remove(),m.delete(H))},E=N=>{if(N.seq<=h||(h=N.seq,N.kind==="model_request"))return;let H=N.data;P(()=>{let $=w(N.work);switch(N.kind){case"work_start":Be($.head,lt(String(H.kind??"work"),"info"),x("p.work-prompt",null,String(H.prompt??"")));break;case"work_end":{let ee=String(H.status??""),J=/fail|error/.test(ee)?"danger":/cancel|stop/.test(ee)?"warn":"ok";$.root.append(x("div.work-end",null,lt(ee||"done",J)));break}case"model_response":T(N.work),$.root.append(...tp(H));break;case"tool_decision":{let ee=np(H);ee&&$.root.append(x("div.decision",null,ee));break}case"tool_result":{let ee=H.is_error===!0,J=String(H.content??H.output??"");$.root.append(x("details.result",null,x("summary",{class:ee?"danger":"ok"},ee?"Error":"Output"),x("pre.pre",null,J)));break}case"error":$.root.append(x("p.danger",null,String(H.message??"Error")));break;default:break}})},U=N=>{let H=N;if(H.cid!==e)return;let $=`${H.work}:${H.req}`;P(()=>{let ee=m.get($);if(!ee){let ie=new Text,ce=new Text,we=x("div.live-turn",null,x("p.prose",null,ie));ee={work:H.work,el:we,text:ie,thinking:ce},m.set($,ee),w(H.work).root.append(we)}let J=H.delta;J.type==="tool_use"?ee.el.append(x("span.chip",null,`\u2192 ${J.name}`)):J.type==="text"&&ee.text.appendData(J.text)})},p=async()=>{let N=await mt(()=>He.transcript(e));if(_){for(let H of(N??[]).slice().sort(($,ee)=>$.seq-ee.seq))E(H);v=!0;for(let H of u.splice(0))H();i.scrollTop=i.scrollHeight}},S=N=>{let H=M??"",$=N.map(ee=>({value:`${ee.provider}/${ee.id}`,label:ee.name??ee.id}));H&&!$.some(ee=>ee.value===H)&&$.push({value:H,label:H}),o.replaceChildren(x("option",{value:""},"Use the default"),...$.map(ee=>x("option",{value:ee.value},ee.label))),o.value=H},y=()=>{let N=g;if(!N)return;let H=N.status==="offline",$=N.status==="working"?"Working":N.status==="idle"?"Idle":"Offline",ee=N.status==="working"?"info":N.status==="idle"?"ok":"muted";Be(t,x("a.back",{href:bn("agents")},"Agents"),x("div.agent-title",null,x("h1",null,mn(N)),lt($,ee,!0),lt(N.tier==="orchestrator"?"Orchestrator":"Worker",N.tier==="orchestrator"?"accent":"muted"),x("span.muted",null,Sn(N.role))),x("div.row.agent-actions",null,N.status==="working"?Ye("Cancel work",()=>{mt(()=>He.cancel(e),"Cancelled")},{icon:"ban"}):null,Ye("Stop",()=>{mt(()=>He.stop(e),"Stopped")},{kind:"danger",icon:"stop"}))),r.disabled=H,s.disabled=H||f;let J=N.model_source==="agent"?"set for this agent":N.model_source==="role"?"from role":"default";Be(a,x("strong",null,N.model),x("span.faint",null,J));let ie=N.model_override??"";ie!==M&&(M=ie,o.value=ie),Be(l,x("dl.kv",null,x("dt",null,"CID"),x("dd.mono",null,String(N.cid)),x("dt",null,"Run"),x("dd.mono",null,N.run??"\u2014"),x("dt",null,"VM"),x("dd.mono",null,N.vm===null?"\u2014":String(N.vm)),x("dt",null,"Connected"),x("dd",null,qt(N.connected_at_ms)),x("dt",null,"Last seen"),x("dd",null,qt(N.last_seen_ms)),x("dt",null,"Tokens in"),x("dd.mono",null,ys(N.usage.input_tokens)),x("dt",null,"Tokens out"),x("dd.mono",null,ys(N.usage.output_tokens)),x("dt",null,"Credits"),x("dd.mono",null,String(N.credits)))),Be(c,N.work?x("div.row",null,lt(N.work.kind,"info"),x("span.truncate",null,jt(N.work.prompt,160))):x("span.faint",null,"Nothing running"),x("span.muted",null,`${N.queue} queued`))},R=()=>g?.status==="offline",I=async()=>{let N=r.value.trim();if(!N||f||R())return;f=!0,s.disabled=!0;let H=await mt(()=>He.chat(e,N));f=!1,s.disabled=R(),H&&(r.value="")};r.addEventListener("keydown",N=>{N.key==="Enter"&&(N.metaKey||N.ctrlKey)&&(N.preventDefault(),I())}),o.addEventListener("change",()=>{let N=o.value;mt(()=>He.setAgentModel(e,N||null),"Model updated").then(H=>{H?M=N:o.value=M??""})});let k=[ft.watch(N=>{let H=N.find($=>$.cid===e);H&&(g=H,y())}),Mt("transcript",N=>{let H=N;H.cid===e&&(v?E(H):u.push(()=>E(H)))}),Mt("delta",N=>{N.cid===e&&(v?U(N):u.push(()=>U(N)))})];return n.append(x("div.agent-view",null,t,x("div.agent-grid",null,x("div.agent-main",null,Gt(null,i,{flush:!0}),x("div.composer",null,r,x("div.row",null,x("span",{style:"flex:1"}),s))),x("aside.agent-side",null,Gt("Model",x("div.stack",null,a,yt("Change model",o))),Gt("Details",l),Gt("Queue",c))))),(async()=>{try{g=await He.agent(e)}catch(N){if(!_)return;Be(n,N instanceof xs&&N.status===404?pt("agents","No such agent",void 0,x("a.btn",{href:bn("agents")},"All agents")):pt("alert","Could not load this agent",N instanceof Error?N.message:void 0));return}_&&(y(),He.models().then(N=>_&&S(N.models)).catch(()=>{}),await p())})(),()=>{_=!1,k.forEach(N=>N()),m.clear()}}function tp(n){let e=Array.isArray(n.content)?n.content:[],t=[];for(let i of e)i.type==="text"?t.push(x("p.prose",null,i.text)):i.type==="tool_use"&&t.push(x("details.tool",null,x("summary.chip",null,`\u2192 ${i.name}`),x("pre.pre",null,JSON.stringify(i.input,null,2))));return t}function np(n){switch(n.decision){case"allow":return lt("ran in VM","info");case"done":return lt("ran on host","muted");case"deny":return lt(`denied: ${String(n.reason??"")}`,"danger");default:return null}}var ip={working:"info",idle:"ok",offline:"muted"},rp={task:"accent",chat:"info",message:"muted",motion:"warn"},$l=16;function Yu(n){let e="",t="all",i=[],r=pn([{value:"",label:"All roles"}]),s=pn([{value:"all",label:"All statuses"},{value:"working",label:"Working"},{value:"idle",label:"Idle"},{value:"offline",label:"Offline"}]),o=x("div"),a=Ye("Spawn agents",()=>_(),{kind:"primary",icon:"plus"});r.addEventListener("change",()=>{e=r.value,m(ft.get())}),s.addEventListener("change",()=>{t=s.value,m(ft.get())});let l=[{label:"Agent",cell:v=>x("div.row.agent-cell",null,li(ip[v.status]),x("div",null,x("strong.truncate",null,mn(v)),x("div.faint.truncate",null,Sn(v.role))))},{label:"Model",cell:v=>x("div.row.agent-model",null,x("span.mono.truncate",null,v.model),v.tier==="orchestrator"?lt("orchestrator","accent"):null,v.model_source!=="default"?lt(v.model_source==="agent"?"own":"role"):null)},{label:"Work",cell:v=>v.work?x("div.row.agent-work",null,lt(v.work.kind,rp[v.work.kind]),x("span.truncate",null,jt(v.work.prompt,56))):x("span.faint",null,"\u2014")},{label:"Tokens",num:!0,cell:v=>ys(v.usage.input_tokens+v.usage.output_tokens)},{label:"",width:"1%",cell:v=>d(v)}];n.append(x("div.view-head",null,x("h1",null,"Agents"),x("div.row.agent-filters",null,r,s,a)),Gt(null,o,{flush:!0}));let c=ft.watch(m);return()=>c();function d(v){return x("div.row.agent-actions",null,Ye("",f=>{f.stopPropagation(),Mn("agents",v.cid)},{kind:"ghost",size:"sm",icon:"conversations",title:"Chat"}),v.status==="working"?Ye("",f=>{f.stopPropagation(),mt(()=>He.cancel(v.cid),"Work cancelled")},{kind:"ghost",size:"sm",icon:"ban",title:"Cancel work"}):null,Ye("",f=>{f.stopPropagation(),mt(()=>He.stop(v.cid),"Session stopped")},{kind:"ghost",size:"sm",icon:"stop",title:"Stop session"}))}function m(v){if(u(v),!v.length){Be(o,pt("agents","No agents connected","Spawn agents to fill the roster.",Ye("Spawn agents",()=>_(),{kind:"primary",icon:"plus"})));return}let f=v.filter(h=>(e===""||h.role===e)&&(t==="all"||h.status===t));if(!f.length){Be(o,pt("search","No agents match","Clear the filters to see the roster.",Ye("Clear filters",g,{kind:"ghost"})));return}Be(o,x("div.table-scroll",null,or(l,f,h=>Mn("agents",h.cid))))}function u(v){let f=[...new Set(v.map(h=>h.role))].sort();f.join(`
`)!==i.join(`
`)&&(i=f,e&&!i.includes(e)&&(e=""),r.replaceChildren(new Option("All roles",""),...i.map(h=>new Option(Sn(h),h))),r.value=e)}function g(){e="",t="all",r.value="",s.value="all",m(ft.get())}function _(){let v=pn(i.map(T=>({value:T,label:T}))),f=Jt({type:"number",min:1,max:$l,value:1}),h=pn([{value:"",label:"Default for the role"}]),M=Ye("Spawn",()=>{w()},{kind:"primary",icon:"plus"}),P=()=>{},w=async()=>{let T=Math.trunc(Number(f.value));if(!v.value){rn("Pick a role","error");return}if(!(T>=1&&T<=$l)){rn(`Count must be 1 to ${$l}`,"error");return}M.disabled=!0;let E=await mt(()=>He.spawnAgents(v.value,T,h.value||void 0),"Spawning\u2026");M.disabled=!1,E!==void 0&&(P(),rr())};mt(()=>He.models()).then(T=>{T&&h.append(...T.models.map(E=>new Option(`${E.provider}/${E.id}`,`${E.provider}/${E.id}`)))}),P=Ln("Spawn agents",x("div.stack",null,yt("Role",v),yt("Count",f),yt("Model",h),x("div.row.modal-actions",null,M)))}}function Zu(n,e){let t=Number(e[0]);return e[0]&&Number.isFinite(t)?$u(n,t):Yu(n)}var sp=300,Ku="3";function Ju(n,e){let t=x("select.board-filter",{"aria-label":"Filter by author"}),i=x("input.board-search",{type:"search",placeholder:"Search subject or body","aria-label":"Search the board"}),r=x("div.board-feed");n.append(x("div.view-head",null,x("h1",null,"Board"),x("div.actions.board-actions",null,t,i)),r);let s=[],o=!1,a=!1,l=!0,c,d="",m="",u=P=>ft.get().filter(w=>w.cid===P.agent_cid).map(mn)[0]??`agent ${P.agent_cid}`,g=()=>{let P=new Map;for(let T of s){let E=String(T.agent_cid);P.has(E)||P.set(E,u(T))}let w=Array.from(P).sort((T,E)=>T[1].localeCompare(E[1]));d&&!P.has(d)&&(d=""),Be(t,x("option",{value:""},"All authors"),w.map(([T,E])=>x("option",{value:T},E))),t.value=d},_=()=>{for(let P of r.querySelectorAll(".board-body")){let w=P.nextElementSibling;w instanceof HTMLElement&&P.scrollHeight>P.clientHeight+1&&(w.hidden=!1)}},v=()=>{if(!o)return;if(a)return Be(r,pt("alert","Could not load the board."));if(g(),!s.length)return Be(r,pt("board","The board is empty."));let P=[...s].sort((E,U)=>U.created_at-E.created_at),w=m.trim().toLowerCase(),T=P.map((E,U)=>({p:E,no:P.length-U})).filter(({p:E})=>!d||String(E.agent_cid)===d).filter(({p:E})=>!w||`${E.subject}
${E.body}`.toLowerCase().includes(w));if(!T.length)return Be(r,pt("board","Nothing on the board matches."));Be(r,T.map(({p:E,no:U})=>op(E,U,u(E)))),_()},f=async()=>{try{s=await He.board(),a=!1}catch{a=!0}o=!0,l&&v()},M=[Mt("board",()=>{window.clearTimeout(c),c=window.setTimeout(()=>{f()},sp)}),ft.watch(()=>v())];return t.addEventListener("change",()=>{d=t.value,v()}),i.addEventListener("input",()=>{m=i.value,v()}),f(),()=>{l=!1,window.clearTimeout(c),M.forEach(P=>P())}}function op(n,e,t){let i=n.body.trim(),r=i&&i!==n.subject.trim()?x("p.board-body",null,i):null;r&&r.style.setProperty("-webkit-line-clamp",Ku);let s=x("button.board-more",{type:"button",hidden:!0,onclick:()=>{if(!r)return;let o=r.classList.toggle("open");r.style.setProperty("-webkit-line-clamp",o?"none":Ku),s.textContent=o?"Show less":"Show more"}},"Show more");return x("article.board-entry",null,x("span.board-no",null,`p${e}`),x("div.board-matter",null,x("div.board-by",null,x("a.board-author",{href:bn("agents",n.agent_cid)},t),x("span.board-role",null,Sn(n.role))),x("h3.board-subject",null,n.subject),r,r?s:null),x("time.board-time",null,qt(n.created_at)))}var ap=300;function ju(n,e){let t=x("div.convo-list"),i=x("div.thread");n.append(x("div.view-head",null,x("h1",null,"Conversations")),x("div.convo",null,t,i));let r=[],s=!1,o=!1,a=!0,l,c=e[0]?lp(e[0]):void 0,d=f=>{let h=ft.get().find(M=>M.cid===f);return h?mn(h):`agent ${f}`},m=f=>{c=f,u(),Mn("conversations",f)},u=()=>{if(!s)return;if(o){Be(t,pt("alert","Could not load conversations.")),Be(i);return}let f=cp(r);if(!f.length){Be(t,pt("conversations","No conversations yet.")),Be(i);return}let h=f.find(M=>M.key===c)??f[0];h&&(c=h.key,Be(t,f.map(M=>up(M,M.key===h.key,d,()=>m(M.key)))),Be(i,...hp(h,d)),i.scrollTop=i.scrollHeight)},g=async()=>{try{r=await He.conversations(),o=!1}catch{o=!0}s=!0,a&&u()},v=[Mt("conversation",()=>{window.clearTimeout(l),l=window.setTimeout(()=>{g()},ap)}),ft.watch(()=>u())];return g(),()=>{a=!1,window.clearTimeout(l),v.forEach(f=>f())}}function lp(n){return n.split("-").map(Number).sort((e,t)=>e-t).join("-")}function cp(n){let e=new Map,t=[...n].sort((i,r)=>i.created_at-r.created_at);for(let i of t){let r=Math.min(i.from_cid,i.to_cid),s=Math.max(i.from_cid,i.to_cid),o=`${r}-${s}`,a=e.get(o)??{key:o,a:r,b:s,messages:[],last:i};a.messages.push(i),a.last=i,e.set(o,a)}return Array.from(e.values()).sort((i,r)=>r.last.created_at-i.last.created_at)}function Yl(n,e){return x("a.agent-link",{href:bn("agents",n)},e(n))}function up(n,e,t,i){let r=x("button.convo-item",{type:"button","aria-current":e?"true":null},x("span.convo-names",null,`${t(n.a)} \xB7 ${t(n.b)}`),x("span.convo-last",null,jt(n.last.text,90)),x("span.convo-meta",null,`${qt(n.last.created_at)} \xB7 ${n.messages.length} messages`));return r.addEventListener("click",i),r}function hp(n,e){let t=n.messages[0]?.from_cid??n.a;return[x("div.thread-head",null,Yl(n.a,e)," \xB7 ",Yl(n.b,e)),...n.messages.map(i=>dp(i,i.from_cid===t?"left":"right",e))]}function dp(n,e,t){return x("div.bubble-row",{class:e},x("div.bubble",null,x("div.bubble-meta",null,Yl(n.from_cid,t),x("span.faint",null,qt(n.created_at)),n.delivered?null:lt("undelivered","warn")),x("p.bubble-text",null,n.text)))}var eh=n=>/owner|await/i.test(n.state);function fp(n){return eh(n)?"warn":/carr|pass|approv/i.test(n.state)?"ok":/reject|fail|den/i.test(n.state)?"danger":/open/i.test(n.state)?"info":"muted"}var pp={yes:"ok",no:"danger",abstain:"muted"},Zl=(n,e)=>`${e>0?n/e*100:0}%`;function th(n,e){let t=x("div.stack");n.append(x("div.view-head",null,x("h1",null,"Council")),t);let i=!0,r=async()=>{try{let o=await He.motions();if(!i)return;Be(t,o.length?o.map(a=>mp(a,()=>{r()})):pt("council","No motions."))}catch(o){i&&Be(t,pt("alert","Council unreachable",o instanceof Error?o.message:String(o)))}},s=Mt("motion",()=>{r()});return r(),()=>{i=!1,s()}}function mp(n,e){let t=n.tally,i=x("div.tally",{role:"img","aria-label":`${t.yes} yes, ${t.no} no, ${t.abstain} abstain of ${t.seats}`},x("span.tally-seg.yes",{style:`width:${Zl(t.yes,t.seats)}`}),x("span.tally-seg.no",{style:`width:${Zl(t.no,t.seats)}`}),x("span.tally-seg.abstain",{style:`width:${Zl(t.abstain,t.seats)}`})),r=n.votes?.length?x("ul.votes",null,n.votes.map(o=>x("li",null,x("span.muted",null,Sn(o.role)),lt(o.vote,pp[o.vote]??"muted"),x("span",null,o.reason)))):null,s=eh(n)?x("div.motion-actions",null,Ye("Reject",()=>Qu(n,"reject",e),{kind:"danger",icon:"x"}),Ye("Approve",()=>Qu(n,"approve",e),{kind:"primary",icon:"check"})):null;return Gt(x("div.motion-head",null,lt(n.kind,"accent"),lt(n.state,fp(n),!0),x("span.faint",null,qt(n.created_at))),x("div.stack",null,x("p.motion-text",null,n.text),i,x("div.tally-legend.faint",null,`${t.yes} yes \xB7 ${t.no} no \xB7 ${t.abstain} abstain \xB7 ${t.voted}/${t.seats}`),r,s))}function Qu(n,e,t){let i=e==="approve"?"Approve":"Reject",r=Kn({rows:3,placeholder:"Reason"}),s=Ye(i,()=>{s.disabled=!0,mt(()=>He.decide(n.id,e,r.value.trim()),`Motion ${e==="approve"?"approved":"rejected"}`).then(a=>{a?(o(),t()):s.disabled=!1})},{kind:e==="approve"?"primary":"danger"}),o=Ln(`${i} motion`,x("div.stack",null,x("p.muted",null,n.text),r,x("div.motion-actions",null,Ye("Cancel",()=>o(),{kind:"ghost"}),s)))}var Yi={LEFT:0,MIDDLE:1,RIGHT:2,ROTATE:0,DOLLY:1,PAN:2},Zi={ROTATE:0,PAN:1,DOLLY_PAN:2,DOLLY_ROTATE:3},Ph=0,Ac=1,Ih=2;var mr=1,Lh=2,rs=3,Ki=0,fn=1,Nn=2,fi=0,ss=1,os=2,Rc=3,Cc=4,kh=5;var gr=100,Dh=101,Nh=102,Uh=103,Fh=104,Oh=200,Bh=201,zh=202,Hh=203,Pc=204,Ic=205,Gh=206,Vh=207,Wh=208,Xh=209,qh=210,$h=211,Yh=212,Zh=213,Kh=214,ea=0,ta=1,na=2,Xr=3,ia=4,ra=5,sa=6,oa=7,Lc=0,Jh=1,jh=2,ni=0,kc=1,Dc=2,Nc=3,Uc=4,Fc=5,Oc=6,Bc=7;var zc=300,Ji=301,xr=302,Ra=303,Ca=304,no=306,dr=1e3,ui=1001,aa=1002,Qt=1003,Qh=1004;var io=1005;var xn=1006,Pa=1007;var ji=1008;var Un=1009,Hc=1010,Gc=1011,as=1012,Ia=1013,ii=1014,$n=1015,ri=1016,La=1017,ka=1018,ls=1020,Vc=35902,Wc=35899,Xc=1021,qc=1022,Fn=1023,hi=1026,Qi=1027,Da=1028,Na=1029,er=1030,Ua=1031;var Fa=1033,ro=33776,so=33777,oo=33778,ao=33779,Oa=35840,Ba=35841,za=35842,Ha=35843,Ga=36196,Va=37492,Wa=37496,Xa=37488,qa=37489,lo=37490,$a=37491,Ya=37808,Za=37809,Ka=37810,Ja=37811,ja=37812,Qa=37813,el=37814,tl=37815,nl=37816,il=37817,rl=37818,sl=37819,ol=37820,al=37821,ll=36492,cl=36494,ul=36495,hl=36283,dl=36284,co=36285,fl=36286;var Cs=2300,la=2301,jo=2302,xc=2303,vc=2400,_c=2401,yc=2402;var ed=3200;var pl=0,td=1,Ai="",kt="srgb",Ps="srgb-linear",Is="linear",St="srgb";var Qo=7680;var nd=519,id=512,rd=513,sd=514,ml=515,od=516,ad=517,gl=518,ld=519,cd=35044;var $c="300 es",ei=2e3,qr=2001;function gp(n){for(let e=n.length-1;e>=0;--e)if(n[e]>=65535)return!0;return!1}function xp(n){return ArrayBuffer.isView(n)&&!(n instanceof DataView)}function Ls(n){return document.createElementNS("http://www.w3.org/1999/xhtml",n)}function ud(){let n=Ls("canvas");return n.style.display="block",n}var nh={},$r=null;function Yc(...n){let e="THREE."+n.shift();$r?$r("log",e,...n):console.log(e,...n)}function hd(n){let e=n[0];if(typeof e=="string"&&e.startsWith("TSL:")){let t=n[1];t&&t.isStackTrace?n[0]+=" "+t.getLocation():n[1]='Stack trace not available. Enable "THREE.Node.captureStackTrace" to capture stack traces.'}return n}function $e(...n){n=hd(n);let e="THREE."+n.shift();if($r)$r("warn",e,...n);else{let t=n[0];t&&t.isStackTrace?console.warn(t.getError(e)):console.warn(e,...n)}}function Ze(...n){n=hd(n);let e="THREE."+n.shift();if($r)$r("error",e,...n);else{let t=n[0];t&&t.isStackTrace?console.error(t.getError(e)):console.error(e,...n)}}function hr(...n){let e=n.join(" ");e in nh||(nh[e]=!0,$e(...n))}function dd(n,e,t){return new Promise(function(i,r){function s(){switch(n.clientWaitSync(e,n.SYNC_FLUSH_COMMANDS_BIT,0)){case n.WAIT_FAILED:r();break;case n.TIMEOUT_EXPIRED:setTimeout(s,t);break;default:i()}}setTimeout(s,t)})}var fd={[ea]:ta,[na]:sa,[ia]:oa,[Xr]:ra,[ta]:ea,[sa]:na,[oa]:ia,[ra]:Xr},ti=class{addEventListener(e,t){this._listeners===void 0&&(this._listeners={});let i=this._listeners;i[e]===void 0&&(i[e]=[]),i[e].indexOf(t)===-1&&i[e].push(t)}hasEventListener(e,t){let i=this._listeners;return i===void 0?!1:i[e]!==void 0&&i[e].indexOf(t)!==-1}removeEventListener(e,t){let i=this._listeners;if(i===void 0)return;let r=i[e];if(r!==void 0){let s=r.indexOf(t);s!==-1&&r.splice(s,1)}}dispatchEvent(e){let t=this._listeners;if(t===void 0)return;let i=t[e.type];if(i!==void 0){e.target=this;let r=i.slice(0);for(let s=0,o=r.length;s<o;s++)r[s].call(this,e);e.target=null}}},wn=["00","01","02","03","04","05","06","07","08","09","0a","0b","0c","0d","0e","0f","10","11","12","13","14","15","16","17","18","19","1a","1b","1c","1d","1e","1f","20","21","22","23","24","25","26","27","28","29","2a","2b","2c","2d","2e","2f","30","31","32","33","34","35","36","37","38","39","3a","3b","3c","3d","3e","3f","40","41","42","43","44","45","46","47","48","49","4a","4b","4c","4d","4e","4f","50","51","52","53","54","55","56","57","58","59","5a","5b","5c","5d","5e","5f","60","61","62","63","64","65","66","67","68","69","6a","6b","6c","6d","6e","6f","70","71","72","73","74","75","76","77","78","79","7a","7b","7c","7d","7e","7f","80","81","82","83","84","85","86","87","88","89","8a","8b","8c","8d","8e","8f","90","91","92","93","94","95","96","97","98","99","9a","9b","9c","9d","9e","9f","a0","a1","a2","a3","a4","a5","a6","a7","a8","a9","aa","ab","ac","ad","ae","af","b0","b1","b2","b3","b4","b5","b6","b7","b8","b9","ba","bb","bc","bd","be","bf","c0","c1","c2","c3","c4","c5","c6","c7","c8","c9","ca","cb","cc","cd","ce","cf","d0","d1","d2","d3","d4","d5","d6","d7","d8","d9","da","db","dc","dd","de","df","e0","e1","e2","e3","e4","e5","e6","e7","e8","e9","ea","eb","ec","ed","ee","ef","f0","f1","f2","f3","f4","f5","f6","f7","f8","f9","fa","fb","fc","fd","fe","ff"],ih=1234567,As=Math.PI/180,Yr=180/Math.PI;function cs(){let n=Math.random()*4294967295|0,e=Math.random()*4294967295|0,t=Math.random()*4294967295|0,i=Math.random()*4294967295|0;return(wn[n&255]+wn[n>>8&255]+wn[n>>16&255]+wn[n>>24&255]+"-"+wn[e&255]+wn[e>>8&255]+"-"+wn[e>>16&15|64]+wn[e>>24&255]+"-"+wn[t&63|128]+wn[t>>8&255]+"-"+wn[t>>16&255]+wn[t>>24&255]+wn[i&255]+wn[i>>8&255]+wn[i>>16&255]+wn[i>>24&255]).toLowerCase()}function at(n,e,t){return Math.max(e,Math.min(t,n))}function Zc(n,e){return(n%e+e)%e}function vp(n,e,t,i,r){return i+(n-e)*(r-i)/(t-e)}function _p(n,e,t){return n!==e?(t-n)/(e-n):0}function Rs(n,e,t){return(1-t)*n+t*e}function yp(n,e,t,i){return Rs(n,e,1-Math.exp(-t*i))}function bp(n,e=1){return e-Math.abs(Zc(n,e*2)-e)}function Mp(n,e,t){return n<=e?0:n>=t?1:(n=(n-e)/(t-e),n*n*(3-2*n))}function Sp(n,e,t){return n<=e?0:n>=t?1:(n=(n-e)/(t-e),n*n*n*(n*(n*6-15)+10))}function wp(n,e){return n+Math.floor(Math.random()*(e-n+1))}function Ep(n,e){return n+Math.random()*(e-n)}function Tp(n){return n*(.5-Math.random())}function Ap(n){n!==void 0&&(ih=n);let e=ih+=1831565813;return e=Math.imul(e^e>>>15,e|1),e^=e+Math.imul(e^e>>>7,e|61),((e^e>>>14)>>>0)/4294967296}function Rp(n){return n*As}function Cp(n){return n*Yr}function Pp(n){return n>0&&Number.isInteger(n)&&2**Math.round(Math.log2(n))===n}function Ip(n){return Math.pow(2,Math.ceil(Math.log(n)/Math.LN2))}function Lp(n){return Math.pow(2,Math.floor(Math.log(n)/Math.LN2))}function kp(n,e,t,i,r){let s=Math.cos,o=Math.sin,a=s(t/2),l=o(t/2),c=s((e+i)/2),d=o((e+i)/2),m=s((e-i)/2),u=o((e-i)/2),g=s((i-e)/2),_=o((i-e)/2);switch(r){case"XYX":n.set(a*d,l*m,l*u,a*c);break;case"YZY":n.set(l*u,a*d,l*m,a*c);break;case"ZXZ":n.set(l*m,l*u,a*d,a*c);break;case"XZX":n.set(a*d,l*_,l*g,a*c);break;case"YXY":n.set(l*g,a*d,l*_,a*c);break;case"ZYZ":n.set(l*_,l*g,a*d,a*c);break;default:$e("MathUtils: .setQuaternionFromProperEuler() encountered an unknown order: "+r)}}function Vr(n,e){switch(e.constructor){case Float32Array:return n;case Uint32Array:return n/4294967295;case Uint16Array:return n/65535;case Uint8Array:case Uint8ClampedArray:return n/255;case Int32Array:return Math.max(n/2147483647,-1);case Int16Array:return Math.max(n/32767,-1);case Int8Array:return Math.max(n/127,-1);default:throw new Error("THREE.MathUtils: Invalid component type.")}}function Cn(n,e){switch(e.constructor){case Float32Array:return n;case Uint32Array:return Math.round(n*4294967295);case Uint16Array:return Math.round(n*65535);case Uint8Array:case Uint8ClampedArray:return Math.round(n*255);case Int32Array:return Math.round(n*2147483647);case Int16Array:return Math.round(n*32767);case Int8Array:return Math.round(n*127);default:throw new Error("THREE.MathUtils: Invalid component type.")}}var Kc={DEG2RAD:As,RAD2DEG:Yr,generateUUID:cs,clamp:at,euclideanModulo:Zc,mapLinear:vp,inverseLerp:_p,lerp:Rs,damp:yp,pingpong:bp,smoothstep:Mp,smootherstep:Sp,randInt:wp,randFloat:Ep,randFloatSpread:Tp,seededRandom:Ap,degToRad:Rp,radToDeg:Cp,isPowerOfTwo:Pp,ceilPowerOfTwo:Ip,floorPowerOfTwo:Lp,setQuaternionFromProperEuler:kp,normalize:Cn,denormalize:Vr},We=class n{static{n.prototype.isVector2=!0}constructor(e=0,t=0){this.x=e,this.y=t}get width(){return this.x}set width(e){this.x=e}get height(){return this.y}set height(e){this.y=e}set(e,t){return this.x=e,this.y=t,this}setScalar(e){return this.x=e,this.y=e,this}setX(e){return this.x=e,this}setY(e){return this.y=e,this}setComponent(e,t){switch(e){case 0:this.x=t;break;case 1:this.y=t;break;default:throw new Error("THREE.Vector2: index is out of range: "+e)}return this}getComponent(e){switch(e){case 0:return this.x;case 1:return this.y;default:throw new Error("THREE.Vector2: index is out of range: "+e)}}clone(){return new this.constructor(this.x,this.y)}copy(e){return this.x=e.x,this.y=e.y,this}add(e){return this.x+=e.x,this.y+=e.y,this}addScalar(e){return this.x+=e,this.y+=e,this}addVectors(e,t){return this.x=e.x+t.x,this.y=e.y+t.y,this}addScaledVector(e,t){return this.x+=e.x*t,this.y+=e.y*t,this}sub(e){return this.x-=e.x,this.y-=e.y,this}subScalar(e){return this.x-=e,this.y-=e,this}subVectors(e,t){return this.x=e.x-t.x,this.y=e.y-t.y,this}multiply(e){return this.x*=e.x,this.y*=e.y,this}multiplyScalar(e){return this.x*=e,this.y*=e,this}divide(e){return this.x/=e.x,this.y/=e.y,this}divideScalar(e){return this.multiplyScalar(1/e)}applyMatrix3(e){let t=this.x,i=this.y,r=e.elements;return this.x=r[0]*t+r[3]*i+r[6],this.y=r[1]*t+r[4]*i+r[7],this}min(e){return this.x=Math.min(this.x,e.x),this.y=Math.min(this.y,e.y),this}max(e){return this.x=Math.max(this.x,e.x),this.y=Math.max(this.y,e.y),this}clamp(e,t){return this.x=at(this.x,e.x,t.x),this.y=at(this.y,e.y,t.y),this}clampScalar(e,t){return this.x=at(this.x,e,t),this.y=at(this.y,e,t),this}clampLength(e,t){let i=this.length();return this.divideScalar(i||1).multiplyScalar(at(i,e,t))}floor(){return this.x=Math.floor(this.x),this.y=Math.floor(this.y),this}ceil(){return this.x=Math.ceil(this.x),this.y=Math.ceil(this.y),this}round(){return this.x=Math.round(this.x),this.y=Math.round(this.y),this}roundToZero(){return this.x=Math.trunc(this.x),this.y=Math.trunc(this.y),this}negate(){return this.x=-this.x,this.y=-this.y,this}dot(e){return this.x*e.x+this.y*e.y}cross(e){return this.x*e.y-this.y*e.x}lengthSq(){return this.x*this.x+this.y*this.y}length(){return Math.sqrt(this.x*this.x+this.y*this.y)}manhattanLength(){return Math.abs(this.x)+Math.abs(this.y)}normalize(){return this.divideScalar(this.length()||1)}angle(){return Math.atan2(-this.y,-this.x)+Math.PI}angleTo(e){let t=Math.sqrt(this.lengthSq()*e.lengthSq());if(t===0)return Math.PI/2;let i=this.dot(e)/t;return Math.acos(at(i,-1,1))}distanceTo(e){return Math.sqrt(this.distanceToSquared(e))}distanceToSquared(e){let t=this.x-e.x,i=this.y-e.y;return t*t+i*i}manhattanDistanceTo(e){return Math.abs(this.x-e.x)+Math.abs(this.y-e.y)}setLength(e){return this.normalize().multiplyScalar(e)}lerp(e,t){return this.x+=(e.x-this.x)*t,this.y+=(e.y-this.y)*t,this}lerpVectors(e,t,i){return this.x=e.x+(t.x-e.x)*i,this.y=e.y+(t.y-e.y)*i,this}equals(e){return e.x===this.x&&e.y===this.y}fromArray(e,t=0){return this.x=e[t],this.y=e[t+1],this}toArray(e=[],t=0){return e[t]=this.x,e[t+1]=this.y,e}fromBufferAttribute(e,t){return this.x=e.getX(t),this.y=e.getY(t),this}rotateAround(e,t){let i=Math.cos(t),r=Math.sin(t),s=this.x-e.x,o=this.y-e.y;return this.x=s*i-o*r+e.x,this.y=s*r+o*i+e.y,this}random(){return this.x=Math.random(),this.y=Math.random(),this}*[Symbol.iterator](){yield this.x,yield this.y}},an=class{constructor(e=0,t=0,i=0,r=1){this.isQuaternion=!0,this._x=e,this._y=t,this._z=i,this._w=r}static slerpFlat(e,t,i,r,s,o,a){let l=i[r+0],c=i[r+1],d=i[r+2],m=i[r+3],u=s[o+0],g=s[o+1],_=s[o+2],v=s[o+3];if(m!==v||l!==u||c!==g||d!==_){let f=l*u+c*g+d*_+m*v;f<0&&(u=-u,g=-g,_=-_,v=-v,f=-f);let h=1-a;if(f<.9995){let M=Math.acos(f),P=Math.sin(M);h=Math.sin(h*M)/P,a=Math.sin(a*M)/P,l=l*h+u*a,c=c*h+g*a,d=d*h+_*a,m=m*h+v*a}else{l=l*h+u*a,c=c*h+g*a,d=d*h+_*a,m=m*h+v*a;let M=1/Math.sqrt(l*l+c*c+d*d+m*m);l*=M,c*=M,d*=M,m*=M}}e[t]=l,e[t+1]=c,e[t+2]=d,e[t+3]=m}static multiplyQuaternionsFlat(e,t,i,r,s,o){let a=i[r],l=i[r+1],c=i[r+2],d=i[r+3],m=s[o],u=s[o+1],g=s[o+2],_=s[o+3];return e[t]=a*_+d*m+l*g-c*u,e[t+1]=l*_+d*u+c*m-a*g,e[t+2]=c*_+d*g+a*u-l*m,e[t+3]=d*_-a*m-l*u-c*g,e}get x(){return this._x}set x(e){this._x=e,this._onChangeCallback()}get y(){return this._y}set y(e){this._y=e,this._onChangeCallback()}get z(){return this._z}set z(e){this._z=e,this._onChangeCallback()}get w(){return this._w}set w(e){this._w=e,this._onChangeCallback()}set(e,t,i,r){return this._x=e,this._y=t,this._z=i,this._w=r,this._onChangeCallback(),this}clone(){return new this.constructor(this._x,this._y,this._z,this._w)}copy(e){return this._x=e.x,this._y=e.y,this._z=e.z,this._w=e.w,this._onChangeCallback(),this}setFromEuler(e,t=!0){let i=e._x,r=e._y,s=e._z,o=e._order,a=Math.cos,l=Math.sin,c=a(i/2),d=a(r/2),m=a(s/2),u=l(i/2),g=l(r/2),_=l(s/2);switch(o){case"XYZ":this._x=u*d*m+c*g*_,this._y=c*g*m-u*d*_,this._z=c*d*_+u*g*m,this._w=c*d*m-u*g*_;break;case"YXZ":this._x=u*d*m+c*g*_,this._y=c*g*m-u*d*_,this._z=c*d*_-u*g*m,this._w=c*d*m+u*g*_;break;case"ZXY":this._x=u*d*m-c*g*_,this._y=c*g*m+u*d*_,this._z=c*d*_+u*g*m,this._w=c*d*m-u*g*_;break;case"ZYX":this._x=u*d*m-c*g*_,this._y=c*g*m+u*d*_,this._z=c*d*_-u*g*m,this._w=c*d*m+u*g*_;break;case"YZX":this._x=u*d*m+c*g*_,this._y=c*g*m+u*d*_,this._z=c*d*_-u*g*m,this._w=c*d*m-u*g*_;break;case"XZY":this._x=u*d*m-c*g*_,this._y=c*g*m-u*d*_,this._z=c*d*_+u*g*m,this._w=c*d*m+u*g*_;break;default:$e("Quaternion: .setFromEuler() encountered an unknown order: "+o)}return t===!0&&this._onChangeCallback(),this}setFromAxisAngle(e,t){let i=t/2,r=Math.sin(i);return this._x=e.x*r,this._y=e.y*r,this._z=e.z*r,this._w=Math.cos(i),this._onChangeCallback(),this}setFromRotationMatrix(e){let t=e.elements,i=t[0],r=t[4],s=t[8],o=t[1],a=t[5],l=t[9],c=t[2],d=t[6],m=t[10],u=i+a+m;if(u>0){let g=.5/Math.sqrt(u+1);this._w=.25/g,this._x=(d-l)*g,this._y=(s-c)*g,this._z=(o-r)*g}else if(i>a&&i>m){let g=2*Math.sqrt(1+i-a-m);this._w=(d-l)/g,this._x=.25*g,this._y=(r+o)/g,this._z=(s+c)/g}else if(a>m){let g=2*Math.sqrt(1+a-i-m);this._w=(s-c)/g,this._x=(r+o)/g,this._y=.25*g,this._z=(l+d)/g}else{let g=2*Math.sqrt(1+m-i-a);this._w=(o-r)/g,this._x=(s+c)/g,this._y=(l+d)/g,this._z=.25*g}return this._onChangeCallback(),this}setFromUnitVectors(e,t){let i=e.dot(t)+1;return i<1e-8?(i=0,Math.abs(e.x)>Math.abs(e.z)?(this._x=-e.y,this._y=e.x,this._z=0,this._w=i):(this._x=0,this._y=-e.z,this._z=e.y,this._w=i)):(this._x=e.y*t.z-e.z*t.y,this._y=e.z*t.x-e.x*t.z,this._z=e.x*t.y-e.y*t.x,this._w=i),this.normalize()}angleTo(e){return 2*Math.acos(Math.abs(at(this.dot(e),-1,1)))}rotateTowards(e,t){let i=this.angleTo(e);if(i===0)return this;let r=Math.min(1,t/i);return this.slerp(e,r),this}identity(){return this.set(0,0,0,1)}invert(){return this.conjugate()}conjugate(){return this._x*=-1,this._y*=-1,this._z*=-1,this._onChangeCallback(),this}dot(e){return this._x*e._x+this._y*e._y+this._z*e._z+this._w*e._w}lengthSq(){return this._x*this._x+this._y*this._y+this._z*this._z+this._w*this._w}length(){return Math.sqrt(this._x*this._x+this._y*this._y+this._z*this._z+this._w*this._w)}normalize(){let e=this.length();return e===0?(this._x=0,this._y=0,this._z=0,this._w=1):(e=1/e,this._x=this._x*e,this._y=this._y*e,this._z=this._z*e,this._w=this._w*e),this._onChangeCallback(),this}multiply(e){return this.multiplyQuaternions(this,e)}premultiply(e){return this.multiplyQuaternions(e,this)}multiplyQuaternions(e,t){let i=e._x,r=e._y,s=e._z,o=e._w,a=t._x,l=t._y,c=t._z,d=t._w;return this._x=i*d+o*a+r*c-s*l,this._y=r*d+o*l+s*a-i*c,this._z=s*d+o*c+i*l-r*a,this._w=o*d-i*a-r*l-s*c,this._onChangeCallback(),this}slerp(e,t){let i=e._x,r=e._y,s=e._z,o=e._w,a=this.dot(e);a<0&&(i=-i,r=-r,s=-s,o=-o,a=-a);let l=1-t;if(a<.9995){let c=Math.acos(a),d=Math.sin(c);l=Math.sin(l*c)/d,t=Math.sin(t*c)/d,this._x=this._x*l+i*t,this._y=this._y*l+r*t,this._z=this._z*l+s*t,this._w=this._w*l+o*t,this._onChangeCallback()}else this._x=this._x*l+i*t,this._y=this._y*l+r*t,this._z=this._z*l+s*t,this._w=this._w*l+o*t,this.normalize();return this}slerpQuaternions(e,t,i){return this.copy(e).slerp(t,i)}random(){let e=2*Math.PI*Math.random(),t=2*Math.PI*Math.random(),i=Math.random(),r=Math.sqrt(1-i),s=Math.sqrt(i);return this.set(r*Math.sin(e),r*Math.cos(e),s*Math.sin(t),s*Math.cos(t))}equals(e){return e._x===this._x&&e._y===this._y&&e._z===this._z&&e._w===this._w}fromArray(e,t=0){return this._x=e[t],this._y=e[t+1],this._z=e[t+2],this._w=e[t+3],this._onChangeCallback(),this}toArray(e=[],t=0){return e[t]=this._x,e[t+1]=this._y,e[t+2]=this._z,e[t+3]=this._w,e}fromBufferAttribute(e,t){return this._x=e.getX(t),this._y=e.getY(t),this._z=e.getZ(t),this._w=e.getW(t),this._onChangeCallback(),this}toJSON(){return this.toArray()}_onChange(e){return this._onChangeCallback=e,this}_onChangeCallback(){}*[Symbol.iterator](){yield this._x,yield this._y,yield this._z,yield this._w}},G=class n{static{n.prototype.isVector3=!0}constructor(e=0,t=0,i=0){this.x=e,this.y=t,this.z=i}set(e,t,i){return i===void 0&&(i=this.z),this.x=e,this.y=t,this.z=i,this}setScalar(e){return this.x=e,this.y=e,this.z=e,this}setX(e){return this.x=e,this}setY(e){return this.y=e,this}setZ(e){return this.z=e,this}setComponent(e,t){switch(e){case 0:this.x=t;break;case 1:this.y=t;break;case 2:this.z=t;break;default:throw new Error("THREE.Vector3: index is out of range: "+e)}return this}getComponent(e){switch(e){case 0:return this.x;case 1:return this.y;case 2:return this.z;default:throw new Error("THREE.Vector3: index is out of range: "+e)}}clone(){return new this.constructor(this.x,this.y,this.z)}copy(e){return this.x=e.x,this.y=e.y,this.z=e.z,this}add(e){return this.x+=e.x,this.y+=e.y,this.z+=e.z,this}addScalar(e){return this.x+=e,this.y+=e,this.z+=e,this}addVectors(e,t){return this.x=e.x+t.x,this.y=e.y+t.y,this.z=e.z+t.z,this}addScaledVector(e,t){return this.x+=e.x*t,this.y+=e.y*t,this.z+=e.z*t,this}sub(e){return this.x-=e.x,this.y-=e.y,this.z-=e.z,this}subScalar(e){return this.x-=e,this.y-=e,this.z-=e,this}subVectors(e,t){return this.x=e.x-t.x,this.y=e.y-t.y,this.z=e.z-t.z,this}multiply(e){return this.x*=e.x,this.y*=e.y,this.z*=e.z,this}multiplyScalar(e){return this.x*=e,this.y*=e,this.z*=e,this}multiplyVectors(e,t){return this.x=e.x*t.x,this.y=e.y*t.y,this.z=e.z*t.z,this}applyEuler(e){return this.applyQuaternion(rh.setFromEuler(e))}applyAxisAngle(e,t){return this.applyQuaternion(rh.setFromAxisAngle(e,t))}applyMatrix3(e){let t=this.x,i=this.y,r=this.z,s=e.elements;return this.x=s[0]*t+s[3]*i+s[6]*r,this.y=s[1]*t+s[4]*i+s[7]*r,this.z=s[2]*t+s[5]*i+s[8]*r,this}applyNormalMatrix(e){return this.applyMatrix3(e).normalize()}applyMatrix4(e){let t=this.x,i=this.y,r=this.z,s=e.elements,o=1/(s[3]*t+s[7]*i+s[11]*r+s[15]);return this.x=(s[0]*t+s[4]*i+s[8]*r+s[12])*o,this.y=(s[1]*t+s[5]*i+s[9]*r+s[13])*o,this.z=(s[2]*t+s[6]*i+s[10]*r+s[14])*o,this}applyQuaternion(e){let t=this.x,i=this.y,r=this.z,s=e.x,o=e.y,a=e.z,l=e.w,c=2*(o*r-a*i),d=2*(a*t-s*r),m=2*(s*i-o*t);return this.x=t+l*c+o*m-a*d,this.y=i+l*d+a*c-s*m,this.z=r+l*m+s*d-o*c,this}project(e){return this.applyMatrix4(e.matrixWorldInverse).applyMatrix4(e.projectionMatrix)}unproject(e){return this.applyMatrix4(e.projectionMatrixInverse).applyMatrix4(e.matrixWorld)}transformDirection(e){let t=this.x,i=this.y,r=this.z,s=e.elements;return this.x=s[0]*t+s[4]*i+s[8]*r,this.y=s[1]*t+s[5]*i+s[9]*r,this.z=s[2]*t+s[6]*i+s[10]*r,this.normalize()}divide(e){return this.x/=e.x,this.y/=e.y,this.z/=e.z,this}divideScalar(e){return this.multiplyScalar(1/e)}min(e){return this.x=Math.min(this.x,e.x),this.y=Math.min(this.y,e.y),this.z=Math.min(this.z,e.z),this}max(e){return this.x=Math.max(this.x,e.x),this.y=Math.max(this.y,e.y),this.z=Math.max(this.z,e.z),this}clamp(e,t){return this.x=at(this.x,e.x,t.x),this.y=at(this.y,e.y,t.y),this.z=at(this.z,e.z,t.z),this}clampScalar(e,t){return this.x=at(this.x,e,t),this.y=at(this.y,e,t),this.z=at(this.z,e,t),this}clampLength(e,t){let i=this.length();return this.divideScalar(i||1).multiplyScalar(at(i,e,t))}floor(){return this.x=Math.floor(this.x),this.y=Math.floor(this.y),this.z=Math.floor(this.z),this}ceil(){return this.x=Math.ceil(this.x),this.y=Math.ceil(this.y),this.z=Math.ceil(this.z),this}round(){return this.x=Math.round(this.x),this.y=Math.round(this.y),this.z=Math.round(this.z),this}roundToZero(){return this.x=Math.trunc(this.x),this.y=Math.trunc(this.y),this.z=Math.trunc(this.z),this}negate(){return this.x=-this.x,this.y=-this.y,this.z=-this.z,this}dot(e){return this.x*e.x+this.y*e.y+this.z*e.z}lengthSq(){return this.x*this.x+this.y*this.y+this.z*this.z}length(){return Math.sqrt(this.x*this.x+this.y*this.y+this.z*this.z)}manhattanLength(){return Math.abs(this.x)+Math.abs(this.y)+Math.abs(this.z)}normalize(){return this.divideScalar(this.length()||1)}setLength(e){return this.normalize().multiplyScalar(e)}lerp(e,t){return this.x+=(e.x-this.x)*t,this.y+=(e.y-this.y)*t,this.z+=(e.z-this.z)*t,this}lerpVectors(e,t,i){return this.x=e.x+(t.x-e.x)*i,this.y=e.y+(t.y-e.y)*i,this.z=e.z+(t.z-e.z)*i,this}cross(e){return this.crossVectors(this,e)}crossVectors(e,t){let i=e.x,r=e.y,s=e.z,o=t.x,a=t.y,l=t.z;return this.x=r*l-s*a,this.y=s*o-i*l,this.z=i*a-r*o,this}projectOnVector(e){let t=e.lengthSq();if(t===0)return this.set(0,0,0);let i=e.dot(this)/t;return this.copy(e).multiplyScalar(i)}projectOnPlane(e){return Kl.copy(this).projectOnVector(e),this.sub(Kl)}reflect(e){return this.sub(Kl.copy(e).multiplyScalar(2*this.dot(e)))}angleTo(e){let t=Math.sqrt(this.lengthSq()*e.lengthSq());if(t===0)return Math.PI/2;let i=this.dot(e)/t;return Math.acos(at(i,-1,1))}distanceTo(e){return Math.sqrt(this.distanceToSquared(e))}distanceToSquared(e){let t=this.x-e.x,i=this.y-e.y,r=this.z-e.z;return t*t+i*i+r*r}manhattanDistanceTo(e){return Math.abs(this.x-e.x)+Math.abs(this.y-e.y)+Math.abs(this.z-e.z)}setFromSpherical(e){return this.setFromSphericalCoords(e.radius,e.phi,e.theta)}setFromSphericalCoords(e,t,i){let r=Math.sin(t)*e;return this.x=r*Math.sin(i),this.y=Math.cos(t)*e,this.z=r*Math.cos(i),this}setFromCylindrical(e){return this.setFromCylindricalCoords(e.radius,e.theta,e.y)}setFromCylindricalCoords(e,t,i){return this.x=e*Math.sin(t),this.y=i,this.z=e*Math.cos(t),this}setFromMatrixPosition(e){let t=e.elements;return this.x=t[12],this.y=t[13],this.z=t[14],this}setFromMatrixScale(e){let t=this.setFromMatrixColumn(e,0).length(),i=this.setFromMatrixColumn(e,1).length(),r=this.setFromMatrixColumn(e,2).length();return this.x=t,this.y=i,this.z=r,this}setFromMatrixColumn(e,t){return this.fromArray(e.elements,t*4)}setFromMatrix3Column(e,t){return this.fromArray(e.elements,t*3)}setFromEuler(e){return this.x=e._x,this.y=e._y,this.z=e._z,this}setFromColor(e){return this.x=e.r,this.y=e.g,this.z=e.b,this}equals(e){return e.x===this.x&&e.y===this.y&&e.z===this.z}fromArray(e,t=0){return this.x=e[t],this.y=e[t+1],this.z=e[t+2],this}toArray(e=[],t=0){return e[t]=this.x,e[t+1]=this.y,e[t+2]=this.z,e}fromBufferAttribute(e,t){return this.x=e.getX(t),this.y=e.getY(t),this.z=e.getZ(t),this}random(){return this.x=Math.random(),this.y=Math.random(),this.z=Math.random(),this}randomDirection(){let e=Math.random()*Math.PI*2,t=Math.random()*2-1,i=Math.sqrt(1-t*t);return this.x=i*Math.cos(e),this.y=t,this.z=i*Math.sin(e),this}*[Symbol.iterator](){yield this.x,yield this.y,yield this.z}},Kl=new G,rh=new an,je=class n{static{n.prototype.isMatrix3=!0}constructor(e,t,i,r,s,o,a,l,c){this.elements=[1,0,0,0,1,0,0,0,1],e!==void 0&&this.set(e,t,i,r,s,o,a,l,c)}set(e,t,i,r,s,o,a,l,c){let d=this.elements;return d[0]=e,d[1]=r,d[2]=a,d[3]=t,d[4]=s,d[5]=l,d[6]=i,d[7]=o,d[8]=c,this}identity(){return this.set(1,0,0,0,1,0,0,0,1),this}copy(e){let t=this.elements,i=e.elements;return t[0]=i[0],t[1]=i[1],t[2]=i[2],t[3]=i[3],t[4]=i[4],t[5]=i[5],t[6]=i[6],t[7]=i[7],t[8]=i[8],this}extractBasis(e,t,i){return e.setFromMatrix3Column(this,0),t.setFromMatrix3Column(this,1),i.setFromMatrix3Column(this,2),this}setFromMatrix4(e){let t=e.elements;return this.set(t[0],t[4],t[8],t[1],t[5],t[9],t[2],t[6],t[10]),this}multiply(e){return this.multiplyMatrices(this,e)}premultiply(e){return this.multiplyMatrices(e,this)}multiplyMatrices(e,t){let i=e.elements,r=t.elements,s=this.elements,o=i[0],a=i[3],l=i[6],c=i[1],d=i[4],m=i[7],u=i[2],g=i[5],_=i[8],v=r[0],f=r[3],h=r[6],M=r[1],P=r[4],w=r[7],T=r[2],E=r[5],U=r[8];return s[0]=o*v+a*M+l*T,s[3]=o*f+a*P+l*E,s[6]=o*h+a*w+l*U,s[1]=c*v+d*M+m*T,s[4]=c*f+d*P+m*E,s[7]=c*h+d*w+m*U,s[2]=u*v+g*M+_*T,s[5]=u*f+g*P+_*E,s[8]=u*h+g*w+_*U,this}multiplyScalar(e){let t=this.elements;return t[0]*=e,t[3]*=e,t[6]*=e,t[1]*=e,t[4]*=e,t[7]*=e,t[2]*=e,t[5]*=e,t[8]*=e,this}determinant(){let e=this.elements,t=e[0],i=e[1],r=e[2],s=e[3],o=e[4],a=e[5],l=e[6],c=e[7],d=e[8];return t*o*d-t*a*c-i*s*d+i*a*l+r*s*c-r*o*l}invert(){let e=this.elements,t=e[0],i=e[1],r=e[2],s=e[3],o=e[4],a=e[5],l=e[6],c=e[7],d=e[8],m=d*o-a*c,u=a*l-d*s,g=c*s-o*l,_=t*m+i*u+r*g;if(_===0)return this.set(0,0,0,0,0,0,0,0,0);let v=1/_;return e[0]=m*v,e[1]=(r*c-d*i)*v,e[2]=(a*i-r*o)*v,e[3]=u*v,e[4]=(d*t-r*l)*v,e[5]=(r*s-a*t)*v,e[6]=g*v,e[7]=(i*l-c*t)*v,e[8]=(o*t-i*s)*v,this}transpose(){let e,t=this.elements;return e=t[1],t[1]=t[3],t[3]=e,e=t[2],t[2]=t[6],t[6]=e,e=t[5],t[5]=t[7],t[7]=e,this}getNormalMatrix(e){return this.setFromMatrix4(e).invert().transpose()}transposeIntoArray(e){let t=this.elements;return e[0]=t[0],e[1]=t[3],e[2]=t[6],e[3]=t[1],e[4]=t[4],e[5]=t[7],e[6]=t[2],e[7]=t[5],e[8]=t[8],this}setUvTransform(e,t,i,r,s,o,a){let l=Math.cos(s),c=Math.sin(s);return this.set(i*l,i*c,-i*(l*o+c*a)+o+e,-r*c,r*l,-r*(-c*o+l*a)+a+t,0,0,1),this}scale(e,t){return hr("Matrix3: .scale() is deprecated. Use .makeScale() instead."),this.premultiply(Jl.makeScale(e,t)),this}rotate(e){return hr("Matrix3: .rotate() is deprecated. Use .makeRotation() instead."),this.premultiply(Jl.makeRotation(-e)),this}translate(e,t){return hr("Matrix3: .translate() is deprecated. Use .makeTranslation() instead."),this.premultiply(Jl.makeTranslation(e,t)),this}makeTranslation(e,t){return e.isVector2?this.set(1,0,e.x,0,1,e.y,0,0,1):this.set(1,0,e,0,1,t,0,0,1),this}makeRotation(e){let t=Math.cos(e),i=Math.sin(e);return this.set(t,-i,0,i,t,0,0,0,1),this}makeScale(e,t){return this.set(e,0,0,0,t,0,0,0,1),this}equals(e){let t=this.elements,i=e.elements;for(let r=0;r<9;r++)if(t[r]!==i[r])return!1;return!0}fromArray(e,t=0){for(let i=0;i<9;i++)this.elements[i]=e[i+t];return this}toArray(e=[],t=0){let i=this.elements;return e[t]=i[0],e[t+1]=i[1],e[t+2]=i[2],e[t+3]=i[3],e[t+4]=i[4],e[t+5]=i[5],e[t+6]=i[6],e[t+7]=i[7],e[t+8]=i[8],e}clone(){return new this.constructor().fromArray(this.elements)}},Jl=new je,sh=new je().set(.4123908,.3575843,.1804808,.212639,.7151687,.0721923,.0193308,.1191948,.9505322),oh=new je().set(3.2409699,-1.5373832,-.4986108,-.9692436,1.8759675,.0415551,.0556301,-.203977,1.0569715);function Dp(){let n={enabled:!0,workingColorSpace:Ps,spaces:{},convert:function(r,s,o){return this.enabled===!1||s===o||!s||!o||(this.spaces[s].transfer===St&&(r.r=wi(r.r),r.g=wi(r.g),r.b=wi(r.b)),this.spaces[s].primaries!==this.spaces[o].primaries&&(r.applyMatrix3(this.spaces[s].toXYZ),r.applyMatrix3(this.spaces[o].fromXYZ)),this.spaces[o].transfer===St&&(r.r=Wr(r.r),r.g=Wr(r.g),r.b=Wr(r.b))),r},workingToColorSpace:function(r,s){return this.convert(r,this.workingColorSpace,s)},colorSpaceToWorking:function(r,s){return this.convert(r,s,this.workingColorSpace)},getPrimaries:function(r){return this.spaces[r].primaries},getTransfer:function(r){return r===Ai?Is:this.spaces[r].transfer},getToneMappingMode:function(r){return this.spaces[r].outputColorSpaceConfig.toneMappingMode||"standard"},getLuminanceCoefficients:function(r,s=this.workingColorSpace){return r.fromArray(this.spaces[s].luminanceCoefficients)},define:function(r){Object.assign(this.spaces,r)},_getMatrix:function(r,s,o){return r.copy(this.spaces[s].toXYZ).multiply(this.spaces[o].fromXYZ)},_getDrawingBufferColorSpace:function(r){return this.spaces[r].outputColorSpaceConfig.drawingBufferColorSpace},_getUnpackColorSpace:function(r=this.workingColorSpace){return this.spaces[r].workingColorSpaceConfig.unpackColorSpace},fromWorkingColorSpace:function(r,s){return hr("ColorManagement: .fromWorkingColorSpace() has been renamed to .workingToColorSpace()."),n.workingToColorSpace(r,s)},toWorkingColorSpace:function(r,s){return hr("ColorManagement: .toWorkingColorSpace() has been renamed to .colorSpaceToWorking()."),n.colorSpaceToWorking(r,s)}},e=[.64,.33,.3,.6,.15,.06],t=[.2126,.7152,.0722],i=[.3127,.329];return n.define({[Ps]:{primaries:e,whitePoint:i,transfer:Is,toXYZ:sh,fromXYZ:oh,luminanceCoefficients:t,workingColorSpaceConfig:{unpackColorSpace:kt},outputColorSpaceConfig:{drawingBufferColorSpace:kt}},[kt]:{primaries:e,whitePoint:i,transfer:St,toXYZ:sh,fromXYZ:oh,luminanceCoefficients:t,outputColorSpaceConfig:{drawingBufferColorSpace:kt}}}),n}var ut=Dp();function wi(n){return n<.04045?n*.0773993808:Math.pow(n*.9478672986+.0521327014,2.4)}function Wr(n){return n<.0031308?n*12.92:1.055*Math.pow(n,.41666)-.055}var Cr,ca=class{static getDataURL(e,t="image/png"){if(/^data:/i.test(e.src)||typeof HTMLCanvasElement>"u")return e.src;let i;if(e instanceof HTMLCanvasElement)i=e;else{Cr===void 0&&(Cr=Ls("canvas")),Cr.width=e.width,Cr.height=e.height;let r=Cr.getContext("2d");e instanceof ImageData?r.putImageData(e,0,0):r.drawImage(e,0,0,e.width,e.height),i=Cr}return i.toDataURL(t)}static sRGBToLinear(e){if(typeof HTMLImageElement<"u"&&e instanceof HTMLImageElement||typeof HTMLCanvasElement<"u"&&e instanceof HTMLCanvasElement||typeof ImageBitmap<"u"&&e instanceof ImageBitmap){let t=Ls("canvas");t.width=e.width,t.height=e.height;let i=t.getContext("2d");i.drawImage(e,0,0,e.width,e.height);let r=i.getImageData(0,0,e.width,e.height),s=r.data;for(let o=0;o<s.length;o++)s[o]=wi(s[o]/255)*255;return i.putImageData(r,0,0),t}else if(e.data){let t=e.data.slice(0);for(let i=0;i<t.length;i++)t instanceof Uint8Array||t instanceof Uint8ClampedArray?t[i]=Math.floor(wi(t[i]/255)*255):t[i]=wi(t[i]);return{data:t,width:e.width,height:e.height}}else return $e("ImageUtils.sRGBToLinear(): Unsupported image type. No color space conversion applied."),e}},Np=0,Zr=class{constructor(e=null){this.isTextureSource=!0,Object.defineProperty(this,"id",{value:Np++}),this.uuid=cs(),this.data=e,this.dataReady=!0,this.version=0}getSize(e){let t=this.data;return typeof HTMLVideoElement<"u"&&t instanceof HTMLVideoElement?e.set(t.videoWidth,t.videoHeight,0):typeof VideoFrame<"u"&&t instanceof VideoFrame?e.set(t.displayWidth,t.displayHeight,0):t!==null?e.set(t.width,t.height,t.depth||0):e.set(0,0,0),e}set needsUpdate(e){e===!0&&this.version++}toJSON(e){let t=e===void 0||typeof e=="string";if(!t&&e.images[this.uuid]!==void 0)return e.images[this.uuid];let i={uuid:this.uuid,url:""},r=this.data;if(r!==null){let s;if(Array.isArray(r)){s=[];for(let o=0,a=r.length;o<a;o++)r[o].isDataTexture?s.push(jl(r[o].image)):s.push(jl(r[o]))}else s=jl(r);i.url=s}return t||(e.images[this.uuid]=i),i}};function jl(n){return typeof HTMLImageElement<"u"&&n instanceof HTMLImageElement||typeof HTMLCanvasElement<"u"&&n instanceof HTMLCanvasElement||typeof ImageBitmap<"u"&&n instanceof ImageBitmap?ca.getDataURL(n):n.data?{data:Array.from(n.data),width:n.width,height:n.height,type:n.data.constructor.name}:($e("Texture: Unable to serialize Texture."),{})}var Up=0,Ql=new G,In=class n extends ti{constructor(e=n.DEFAULT_IMAGE,t=n.DEFAULT_MAPPING,i=ui,r=ui,s=xn,o=ji,a=Fn,l=Un,c=n.DEFAULT_ANISOTROPY,d=Ai){super(),this.isTexture=!0,Object.defineProperty(this,"id",{value:Up++}),this.uuid=cs(),this.name="",this.source=new Zr(e),this.mipmaps=[],this.mapping=t,this.channel=0,this.wrapS=i,this.wrapT=r,this.magFilter=s,this.minFilter=o,this.anisotropy=c,this.format=a,this.internalFormat=null,this.type=l,this.offset=new We(0,0),this.repeat=new We(1,1),this.center=new We(0,0),this.rotation=0,this.matrixAutoUpdate=!0,this.matrix=new je,this.generateMipmaps=!0,this.premultiplyAlpha=!1,this.flipY=!0,this.unpackAlignment=4,this.colorSpace=d,this.userData={},this.updateRanges=[],this.version=0,this.onUpdate=null,this.renderTarget=null,this.isRenderTargetTexture=!1,this.isArrayTexture=!!(e&&e.depth&&e.depth>1),this.pmremVersion=0,this.normalized=!1}get width(){return this.source.getSize(Ql).x}get height(){return this.source.getSize(Ql).y}get depth(){return this.source.getSize(Ql).z}get image(){return this.source.data}set image(e){this.source.data=e}updateMatrix(){this.matrix.setUvTransform(this.offset.x,this.offset.y,this.repeat.x,this.repeat.y,this.rotation,this.center.x,this.center.y)}addUpdateRange(e,t){this.updateRanges.push({start:e,count:t})}clearUpdateRanges(){this.updateRanges.length=0}clone(){return new this.constructor().copy(this)}copy(e){return this.name=e.name,this.source=e.source,this.mipmaps=e.mipmaps.slice(0),this.mapping=e.mapping,this.channel=e.channel,this.wrapS=e.wrapS,this.wrapT=e.wrapT,this.magFilter=e.magFilter,this.minFilter=e.minFilter,this.anisotropy=e.anisotropy,this.format=e.format,this.internalFormat=e.internalFormat,this.type=e.type,this.normalized=e.normalized,this.offset.copy(e.offset),this.repeat.copy(e.repeat),this.center.copy(e.center),this.rotation=e.rotation,this.matrixAutoUpdate=e.matrixAutoUpdate,this.matrix.copy(e.matrix),this.generateMipmaps=e.generateMipmaps,this.premultiplyAlpha=e.premultiplyAlpha,this.flipY=e.flipY,this.unpackAlignment=e.unpackAlignment,this.colorSpace=e.colorSpace,this.renderTarget=e.renderTarget,this.isRenderTargetTexture=e.isRenderTargetTexture,this.isArrayTexture=e.isArrayTexture,this.userData=JSON.parse(JSON.stringify(e.userData)),this.needsUpdate=!0,this}setValues(e){for(let t in e){let i=e[t];if(i===void 0){$e(`Texture.setValues(): parameter '${t}' has value of undefined.`);continue}let r=this[t];if(r===void 0){$e(`Texture.setValues(): property '${t}' does not exist.`);continue}r&&i&&r.isVector2&&i.isVector2||r&&i&&r.isVector3&&i.isVector3||r&&i&&r.isMatrix3&&i.isMatrix3?r.copy(i):this[t]=i}}toJSON(e){let t=e===void 0||typeof e=="string";if(!t&&e.textures[this.uuid]!==void 0)return e.textures[this.uuid];let i={metadata:{version:4.7,type:"Texture",generator:"Texture.toJSON"},uuid:this.uuid,name:this.name,image:this.source.toJSON(e).uuid,mapping:this.mapping,channel:this.channel,repeat:[this.repeat.x,this.repeat.y],offset:[this.offset.x,this.offset.y],center:[this.center.x,this.center.y],rotation:this.rotation,wrap:[this.wrapS,this.wrapT],format:this.format,internalFormat:this.internalFormat,type:this.type,normalized:this.normalized,colorSpace:this.colorSpace,minFilter:this.minFilter,magFilter:this.magFilter,anisotropy:this.anisotropy,flipY:this.flipY,generateMipmaps:this.generateMipmaps,premultiplyAlpha:this.premultiplyAlpha,unpackAlignment:this.unpackAlignment};return Object.keys(this.userData).length>0&&(i.userData=this.userData),t||(e.textures[this.uuid]=i),i}dispose(){this.dispatchEvent({type:"dispose"})}transformUv(e){if(this.mapping!==zc)return e;if(e.applyMatrix3(this.matrix),e.x<0||e.x>1)switch(this.wrapS){case dr:e.x=e.x-Math.floor(e.x);break;case ui:e.x=e.x<0?0:1;break;case aa:Math.abs(Math.floor(e.x)%2)===1?e.x=Math.ceil(e.x)-e.x:e.x=e.x-Math.floor(e.x);break}if(e.y<0||e.y>1)switch(this.wrapT){case dr:e.y=e.y-Math.floor(e.y);break;case ui:e.y=e.y<0?0:1;break;case aa:Math.abs(Math.floor(e.y)%2)===1?e.y=Math.ceil(e.y)-e.y:e.y=e.y-Math.floor(e.y);break}return this.flipY&&(e.y=1-e.y),e}set needsUpdate(e){e===!0&&(this.version++,this.source.needsUpdate=!0)}set needsPMREMUpdate(e){e===!0&&this.pmremVersion++}};In.DEFAULT_IMAGE=null;In.DEFAULT_MAPPING=zc;In.DEFAULT_ANISOTROPY=1;var Bt=class n{static{n.prototype.isVector4=!0}constructor(e=0,t=0,i=0,r=1){this.x=e,this.y=t,this.z=i,this.w=r}get width(){return this.z}set width(e){this.z=e}get height(){return this.w}set height(e){this.w=e}set(e,t,i,r){return this.x=e,this.y=t,this.z=i,this.w=r,this}setScalar(e){return this.x=e,this.y=e,this.z=e,this.w=e,this}setX(e){return this.x=e,this}setY(e){return this.y=e,this}setZ(e){return this.z=e,this}setW(e){return this.w=e,this}setComponent(e,t){switch(e){case 0:this.x=t;break;case 1:this.y=t;break;case 2:this.z=t;break;case 3:this.w=t;break;default:throw new Error("THREE.Vector4: index is out of range: "+e)}return this}getComponent(e){switch(e){case 0:return this.x;case 1:return this.y;case 2:return this.z;case 3:return this.w;default:throw new Error("THREE.Vector4: index is out of range: "+e)}}clone(){return new this.constructor(this.x,this.y,this.z,this.w)}copy(e){return this.x=e.x,this.y=e.y,this.z=e.z,this.w=e.w!==void 0?e.w:1,this}add(e){return this.x+=e.x,this.y+=e.y,this.z+=e.z,this.w+=e.w,this}addScalar(e){return this.x+=e,this.y+=e,this.z+=e,this.w+=e,this}addVectors(e,t){return this.x=e.x+t.x,this.y=e.y+t.y,this.z=e.z+t.z,this.w=e.w+t.w,this}addScaledVector(e,t){return this.x+=e.x*t,this.y+=e.y*t,this.z+=e.z*t,this.w+=e.w*t,this}sub(e){return this.x-=e.x,this.y-=e.y,this.z-=e.z,this.w-=e.w,this}subScalar(e){return this.x-=e,this.y-=e,this.z-=e,this.w-=e,this}subVectors(e,t){return this.x=e.x-t.x,this.y=e.y-t.y,this.z=e.z-t.z,this.w=e.w-t.w,this}multiply(e){return this.x*=e.x,this.y*=e.y,this.z*=e.z,this.w*=e.w,this}multiplyScalar(e){return this.x*=e,this.y*=e,this.z*=e,this.w*=e,this}applyMatrix4(e){let t=this.x,i=this.y,r=this.z,s=this.w,o=e.elements;return this.x=o[0]*t+o[4]*i+o[8]*r+o[12]*s,this.y=o[1]*t+o[5]*i+o[9]*r+o[13]*s,this.z=o[2]*t+o[6]*i+o[10]*r+o[14]*s,this.w=o[3]*t+o[7]*i+o[11]*r+o[15]*s,this}divide(e){return this.x/=e.x,this.y/=e.y,this.z/=e.z,this.w/=e.w,this}divideScalar(e){return this.multiplyScalar(1/e)}setAxisAngleFromQuaternion(e){this.w=2*Math.acos(e.w);let t=Math.sqrt(1-e.w*e.w);return t<1e-4?(this.x=1,this.y=0,this.z=0):(this.x=e.x/t,this.y=e.y/t,this.z=e.z/t),this}setAxisAngleFromRotationMatrix(e){let t,i,r,s,l=e.elements,c=l[0],d=l[4],m=l[8],u=l[1],g=l[5],_=l[9],v=l[2],f=l[6],h=l[10];if(Math.abs(d-u)<.01&&Math.abs(m-v)<.01&&Math.abs(_-f)<.01){if(Math.abs(d+u)<.1&&Math.abs(m+v)<.1&&Math.abs(_+f)<.1&&Math.abs(c+g+h-3)<.1)return this.set(1,0,0,0),this;t=Math.PI;let P=(c+1)/2,w=(g+1)/2,T=(h+1)/2,E=(d+u)/4,U=(m+v)/4,p=(_+f)/4;return P>w&&P>T?P<.01?(i=0,r=.707106781,s=.707106781):(i=Math.sqrt(P),r=E/i,s=U/i):w>T?w<.01?(i=.707106781,r=0,s=.707106781):(r=Math.sqrt(w),i=E/r,s=p/r):T<.01?(i=.707106781,r=.707106781,s=0):(s=Math.sqrt(T),i=U/s,r=p/s),this.set(i,r,s,t),this}let M=Math.sqrt((f-_)*(f-_)+(m-v)*(m-v)+(u-d)*(u-d));return Math.abs(M)<.001&&(M=1),this.x=(f-_)/M,this.y=(m-v)/M,this.z=(u-d)/M,this.w=Math.acos((c+g+h-1)/2),this}setFromMatrixPosition(e){let t=e.elements;return this.x=t[12],this.y=t[13],this.z=t[14],this.w=t[15],this}min(e){return this.x=Math.min(this.x,e.x),this.y=Math.min(this.y,e.y),this.z=Math.min(this.z,e.z),this.w=Math.min(this.w,e.w),this}max(e){return this.x=Math.max(this.x,e.x),this.y=Math.max(this.y,e.y),this.z=Math.max(this.z,e.z),this.w=Math.max(this.w,e.w),this}clamp(e,t){return this.x=at(this.x,e.x,t.x),this.y=at(this.y,e.y,t.y),this.z=at(this.z,e.z,t.z),this.w=at(this.w,e.w,t.w),this}clampScalar(e,t){return this.x=at(this.x,e,t),this.y=at(this.y,e,t),this.z=at(this.z,e,t),this.w=at(this.w,e,t),this}clampLength(e,t){let i=this.length();return this.divideScalar(i||1).multiplyScalar(at(i,e,t))}floor(){return this.x=Math.floor(this.x),this.y=Math.floor(this.y),this.z=Math.floor(this.z),this.w=Math.floor(this.w),this}ceil(){return this.x=Math.ceil(this.x),this.y=Math.ceil(this.y),this.z=Math.ceil(this.z),this.w=Math.ceil(this.w),this}round(){return this.x=Math.round(this.x),this.y=Math.round(this.y),this.z=Math.round(this.z),this.w=Math.round(this.w),this}roundToZero(){return this.x=Math.trunc(this.x),this.y=Math.trunc(this.y),this.z=Math.trunc(this.z),this.w=Math.trunc(this.w),this}negate(){return this.x=-this.x,this.y=-this.y,this.z=-this.z,this.w=-this.w,this}dot(e){return this.x*e.x+this.y*e.y+this.z*e.z+this.w*e.w}lengthSq(){return this.x*this.x+this.y*this.y+this.z*this.z+this.w*this.w}length(){return Math.sqrt(this.x*this.x+this.y*this.y+this.z*this.z+this.w*this.w)}manhattanLength(){return Math.abs(this.x)+Math.abs(this.y)+Math.abs(this.z)+Math.abs(this.w)}normalize(){return this.divideScalar(this.length()||1)}setLength(e){return this.normalize().multiplyScalar(e)}lerp(e,t){return this.x+=(e.x-this.x)*t,this.y+=(e.y-this.y)*t,this.z+=(e.z-this.z)*t,this.w+=(e.w-this.w)*t,this}lerpVectors(e,t,i){return this.x=e.x+(t.x-e.x)*i,this.y=e.y+(t.y-e.y)*i,this.z=e.z+(t.z-e.z)*i,this.w=e.w+(t.w-e.w)*i,this}equals(e){return e.x===this.x&&e.y===this.y&&e.z===this.z&&e.w===this.w}fromArray(e,t=0){return this.x=e[t],this.y=e[t+1],this.z=e[t+2],this.w=e[t+3],this}toArray(e=[],t=0){return e[t]=this.x,e[t+1]=this.y,e[t+2]=this.z,e[t+3]=this.w,e}fromBufferAttribute(e,t){return this.x=e.getX(t),this.y=e.getY(t),this.z=e.getZ(t),this.w=e.getW(t),this}random(){return this.x=Math.random(),this.y=Math.random(),this.z=Math.random(),this.w=Math.random(),this}*[Symbol.iterator](){yield this.x,yield this.y,yield this.z,yield this.w}},ua=class extends ti{constructor(e=1,t=1,i={}){super(),i=Object.assign({generateMipmaps:!1,internalFormat:null,minFilter:xn,depthBuffer:!0,stencilBuffer:!1,resolveColorBuffer:!0,resolveDepthBuffer:!0,resolveStencilBuffer:!0,storeMultisampledColorBuffer:!0,storeMultisampledDepthBuffer:!0,storeMultisampledStencilBuffer:!0,depthTexture:null,samples:0,count:1,depth:1,multiview:!1,useArrayDepthTexture:!1},i),this.isRenderTarget=!0,this.width=e,this.height=t,this.depth=i.depth,this.scissor=new Bt(0,0,e,t),this.scissorTest=!1,this.viewport=new Bt(0,0,e,t),this.textures=[];let r={width:e,height:t,depth:i.depth},s=new In(r),o=i.count;for(let a=0;a<o;a++)this.textures[a]=s.clone(),this.textures[a].isRenderTargetTexture=!0,this.textures[a].renderTarget=this;this._setTextureOptions(i),this.depthBuffer=i.depthBuffer,this.stencilBuffer=i.stencilBuffer,this.resolveColorBuffer=i.resolveColorBuffer,this.resolveDepthBuffer=i.resolveDepthBuffer,this.resolveStencilBuffer=i.resolveStencilBuffer,this.storeMultisampledColorBuffer=i.storeMultisampledColorBuffer,this.storeMultisampledDepthBuffer=i.storeMultisampledDepthBuffer,this.storeMultisampledStencilBuffer=i.storeMultisampledStencilBuffer,this._depthTexture=null,this.depthTexture=i.depthTexture,this.samples=i.samples,this.multiview=i.multiview,this.useArrayDepthTexture=i.useArrayDepthTexture}_setTextureOptions(e={}){let t={minFilter:xn,generateMipmaps:!1,flipY:!1,internalFormat:null};e.mapping!==void 0&&(t.mapping=e.mapping),e.wrapS!==void 0&&(t.wrapS=e.wrapS),e.wrapT!==void 0&&(t.wrapT=e.wrapT),e.wrapR!==void 0&&(t.wrapR=e.wrapR),e.magFilter!==void 0&&(t.magFilter=e.magFilter),e.minFilter!==void 0&&(t.minFilter=e.minFilter),e.format!==void 0&&(t.format=e.format),e.type!==void 0&&(t.type=e.type),e.anisotropy!==void 0&&(t.anisotropy=e.anisotropy),e.colorSpace!==void 0&&(t.colorSpace=e.colorSpace),e.flipY!==void 0&&(t.flipY=e.flipY),e.generateMipmaps!==void 0&&(t.generateMipmaps=e.generateMipmaps),e.internalFormat!==void 0&&(t.internalFormat=e.internalFormat);for(let i=0;i<this.textures.length;i++)this.textures[i].setValues(t)}get texture(){return this.textures[0]}set texture(e){this.textures[0]=e}set depthTexture(e){this._depthTexture!==null&&this._depthTexture.renderTarget===this&&(this._depthTexture.renderTarget=null),e!==null&&e.renderTarget===null&&(e.renderTarget=this),this._depthTexture=e}get depthTexture(){return this._depthTexture}setSize(e,t,i=1){if(this.width!==e||this.height!==t||this.depth!==i){this.width=e,this.height=t,this.depth=i;for(let r=0,s=this.textures.length;r<s;r++)this.textures[r].image.width=e,this.textures[r].image.height=t,this.textures[r].image.depth=i,this.textures[r].isData3DTexture!==!0&&(this.textures[r].isArrayTexture=this.textures[r].image.depth>1);this.dispose()}this.viewport.set(0,0,e,t),this.scissor.set(0,0,e,t)}clone(){return new this.constructor().copy(this)}copy(e){this.width=e.width,this.height=e.height,this.depth=e.depth,this.scissor.copy(e.scissor),this.scissorTest=e.scissorTest,this.viewport.copy(e.viewport),this.textures.length=0;for(let t=0,i=e.textures.length;t<i;t++){this.textures[t]=e.textures[t].clone(),this.textures[t].isRenderTargetTexture=!0,this.textures[t].renderTarget=this;let r=Object.assign({},e.textures[t].image);this.textures[t].source=new Zr(r)}if(this.depthBuffer=e.depthBuffer,this.stencilBuffer=e.stencilBuffer,this.resolveColorBuffer=e.resolveColorBuffer,this.resolveDepthBuffer=e.resolveDepthBuffer,this.resolveStencilBuffer=e.resolveStencilBuffer,this.storeMultisampledColorBuffer=e.storeMultisampledColorBuffer,this.storeMultisampledDepthBuffer=e.storeMultisampledDepthBuffer,this.storeMultisampledStencilBuffer=e.storeMultisampledStencilBuffer,e.depthTexture!==null)if(e.depthTexture.renderTarget===e){let t=e.depthTexture.clone();t.renderTarget=null,this.depthTexture=t}else this.depthTexture=e.depthTexture;return this.samples=e.samples,this.multiview=e.multiview,this.useArrayDepthTexture=e.useArrayDepthTexture,this}dispose(){this.dispatchEvent({type:"dispose"})}},kn=class extends ua{constructor(e=1,t=1,i={}){super(e,t,i),this.isWebGLRenderTarget=!0}},ks=class extends In{constructor(e=null,t=1,i=1,r=1){super(null),this.isDataArrayTexture=!0,this.image={data:e,width:t,height:i,depth:r},this.magFilter=Qt,this.minFilter=Qt,this.wrapR=ui,this.generateMipmaps=!1,this.flipY=!1,this.unpackAlignment=1,this.layerUpdates=new Set}copy(e){return super.copy(e),this.wrapR=e.wrapR,this}addLayerUpdate(e){this.layerUpdates.add(e)}clearLayerUpdates(){this.layerUpdates.clear()}};var ha=class extends In{constructor(e=null,t=1,i=1,r=1){super(null),this.isData3DTexture=!0,this.image={data:e,width:t,height:i,depth:r},this.magFilter=Qt,this.minFilter=Qt,this.wrapR=ui,this.generateMipmaps=!1,this.flipY=!1,this.unpackAlignment=1}copy(e){return super.copy(e),this.wrapR=e.wrapR,this}};var wt=class n{static{n.prototype.isMatrix4=!0}constructor(e,t,i,r,s,o,a,l,c,d,m,u,g,_,v,f){this.elements=[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],e!==void 0&&this.set(e,t,i,r,s,o,a,l,c,d,m,u,g,_,v,f)}set(e,t,i,r,s,o,a,l,c,d,m,u,g,_,v,f){let h=this.elements;return h[0]=e,h[4]=t,h[8]=i,h[12]=r,h[1]=s,h[5]=o,h[9]=a,h[13]=l,h[2]=c,h[6]=d,h[10]=m,h[14]=u,h[3]=g,h[7]=_,h[11]=v,h[15]=f,this}identity(){return this.set(1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1),this}clone(){return new n().fromArray(this.elements)}copy(e){let t=this.elements,i=e.elements;return t[0]=i[0],t[1]=i[1],t[2]=i[2],t[3]=i[3],t[4]=i[4],t[5]=i[5],t[6]=i[6],t[7]=i[7],t[8]=i[8],t[9]=i[9],t[10]=i[10],t[11]=i[11],t[12]=i[12],t[13]=i[13],t[14]=i[14],t[15]=i[15],this}copyPosition(e){let t=this.elements,i=e.elements;return t[12]=i[12],t[13]=i[13],t[14]=i[14],this}setFromMatrix3(e){let t=e.elements;return this.set(t[0],t[3],t[6],0,t[1],t[4],t[7],0,t[2],t[5],t[8],0,0,0,0,1),this}extractBasis(e,t,i){return this.determinantAffine()===0?(e.set(1,0,0),t.set(0,1,0),i.set(0,0,1),this):(e.setFromMatrixColumn(this,0),t.setFromMatrixColumn(this,1),i.setFromMatrixColumn(this,2),this)}makeBasis(e,t,i){return this.set(e.x,t.x,i.x,0,e.y,t.y,i.y,0,e.z,t.z,i.z,0,0,0,0,1),this}extractRotation(e){if(e.determinantAffine()===0)return this.identity();let t=this.elements,i=e.elements,r=1/Pr.setFromMatrixColumn(e,0).length(),s=1/Pr.setFromMatrixColumn(e,1).length(),o=1/Pr.setFromMatrixColumn(e,2).length();return t[0]=i[0]*r,t[1]=i[1]*r,t[2]=i[2]*r,t[3]=0,t[4]=i[4]*s,t[5]=i[5]*s,t[6]=i[6]*s,t[7]=0,t[8]=i[8]*o,t[9]=i[9]*o,t[10]=i[10]*o,t[11]=0,t[12]=0,t[13]=0,t[14]=0,t[15]=1,this}makeRotationFromEuler(e){let t=this.elements,i=e.x,r=e.y,s=e.z,o=Math.cos(i),a=Math.sin(i),l=Math.cos(r),c=Math.sin(r),d=Math.cos(s),m=Math.sin(s);if(e.order==="XYZ"){let u=o*d,g=o*m,_=a*d,v=a*m;t[0]=l*d,t[4]=-l*m,t[8]=c,t[1]=g+_*c,t[5]=u-v*c,t[9]=-a*l,t[2]=v-u*c,t[6]=_+g*c,t[10]=o*l}else if(e.order==="YXZ"){let u=l*d,g=l*m,_=c*d,v=c*m;t[0]=u+v*a,t[4]=_*a-g,t[8]=o*c,t[1]=o*m,t[5]=o*d,t[9]=-a,t[2]=g*a-_,t[6]=v+u*a,t[10]=o*l}else if(e.order==="ZXY"){let u=l*d,g=l*m,_=c*d,v=c*m;t[0]=u-v*a,t[4]=-o*m,t[8]=_+g*a,t[1]=g+_*a,t[5]=o*d,t[9]=v-u*a,t[2]=-o*c,t[6]=a,t[10]=o*l}else if(e.order==="ZYX"){let u=o*d,g=o*m,_=a*d,v=a*m;t[0]=l*d,t[4]=_*c-g,t[8]=u*c+v,t[1]=l*m,t[5]=v*c+u,t[9]=g*c-_,t[2]=-c,t[6]=a*l,t[10]=o*l}else if(e.order==="YZX"){let u=o*l,g=o*c,_=a*l,v=a*c;t[0]=l*d,t[4]=v-u*m,t[8]=_*m+g,t[1]=m,t[5]=o*d,t[9]=-a*d,t[2]=-c*d,t[6]=g*m+_,t[10]=u-v*m}else if(e.order==="XZY"){let u=o*l,g=o*c,_=a*l,v=a*c;t[0]=l*d,t[4]=-m,t[8]=c*d,t[1]=u*m+v,t[5]=o*d,t[9]=g*m-_,t[2]=_*m-g,t[6]=a*d,t[10]=v*m+u}return t[3]=0,t[7]=0,t[11]=0,t[12]=0,t[13]=0,t[14]=0,t[15]=1,this}makeRotationFromQuaternion(e){return this.compose(Fp,e,Op)}lookAt(e,t,i){let r=this.elements;return Bn.subVectors(e,t),Bn.lengthSq()===0&&(Bn.z=1),Bn.normalize(),Di.crossVectors(i,Bn),Di.lengthSq()===0&&(Math.abs(i.z)===1?Bn.x+=1e-4:Bn.z+=1e-4,Bn.normalize(),Di.crossVectors(i,Bn)),Di.normalize(),Po.crossVectors(Bn,Di),r[0]=Di.x,r[4]=Po.x,r[8]=Bn.x,r[1]=Di.y,r[5]=Po.y,r[9]=Bn.y,r[2]=Di.z,r[6]=Po.z,r[10]=Bn.z,this}multiply(e){return this.multiplyMatrices(this,e)}premultiply(e){return this.multiplyMatrices(e,this)}multiplyMatrices(e,t){let i=e.elements,r=t.elements,s=this.elements,o=i[0],a=i[4],l=i[8],c=i[12],d=i[1],m=i[5],u=i[9],g=i[13],_=i[2],v=i[6],f=i[10],h=i[14],M=i[3],P=i[7],w=i[11],T=i[15],E=r[0],U=r[4],p=r[8],S=r[12],y=r[1],R=r[5],I=r[9],k=r[13],D=r[2],N=r[6],H=r[10],$=r[14],ee=r[3],J=r[7],ie=r[11],ce=r[15];return s[0]=o*E+a*y+l*D+c*ee,s[4]=o*U+a*R+l*N+c*J,s[8]=o*p+a*I+l*H+c*ie,s[12]=o*S+a*k+l*$+c*ce,s[1]=d*E+m*y+u*D+g*ee,s[5]=d*U+m*R+u*N+g*J,s[9]=d*p+m*I+u*H+g*ie,s[13]=d*S+m*k+u*$+g*ce,s[2]=_*E+v*y+f*D+h*ee,s[6]=_*U+v*R+f*N+h*J,s[10]=_*p+v*I+f*H+h*ie,s[14]=_*S+v*k+f*$+h*ce,s[3]=M*E+P*y+w*D+T*ee,s[7]=M*U+P*R+w*N+T*J,s[11]=M*p+P*I+w*H+T*ie,s[15]=M*S+P*k+w*$+T*ce,this}multiplyScalar(e){let t=this.elements;return t[0]*=e,t[4]*=e,t[8]*=e,t[12]*=e,t[1]*=e,t[5]*=e,t[9]*=e,t[13]*=e,t[2]*=e,t[6]*=e,t[10]*=e,t[14]*=e,t[3]*=e,t[7]*=e,t[11]*=e,t[15]*=e,this}determinant(){let e=this.elements,t=e[0],i=e[4],r=e[8],s=e[12],o=e[1],a=e[5],l=e[9],c=e[13],d=e[2],m=e[6],u=e[10],g=e[14],_=e[3],v=e[7],f=e[11],h=e[15],M=l*g-c*u,P=a*g-c*m,w=a*u-l*m,T=o*g-c*d,E=o*u-l*d,U=o*m-a*d;return t*(v*M-f*P+h*w)-i*(_*M-f*T+h*E)+r*(_*P-v*T+h*U)-s*(_*w-v*E+f*U)}determinantAffine(){let e=this.elements,t=e[0],i=e[4],r=e[8],s=e[1],o=e[5],a=e[9],l=e[2],c=e[6],d=e[10];return t*(o*d-a*c)-i*(s*d-a*l)+r*(s*c-o*l)}transpose(){let e=this.elements,t;return t=e[1],e[1]=e[4],e[4]=t,t=e[2],e[2]=e[8],e[8]=t,t=e[6],e[6]=e[9],e[9]=t,t=e[3],e[3]=e[12],e[12]=t,t=e[7],e[7]=e[13],e[13]=t,t=e[11],e[11]=e[14],e[14]=t,this}setPosition(e,t,i){let r=this.elements;return e.isVector3?(r[12]=e.x,r[13]=e.y,r[14]=e.z):(r[12]=e,r[13]=t,r[14]=i),this}invert(){let e=this.elements,t=e[0],i=e[1],r=e[2],s=e[3],o=e[4],a=e[5],l=e[6],c=e[7],d=e[8],m=e[9],u=e[10],g=e[11],_=e[12],v=e[13],f=e[14],h=e[15],M=t*a-i*o,P=t*l-r*o,w=t*c-s*o,T=i*l-r*a,E=i*c-s*a,U=r*c-s*l,p=d*v-m*_,S=d*f-u*_,y=d*h-g*_,R=m*f-u*v,I=m*h-g*v,k=u*h-g*f,D=M*k-P*I+w*R+T*y-E*S+U*p;if(D===0)return this.set(0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0);let N=1/D;return e[0]=(a*k-l*I+c*R)*N,e[1]=(r*I-i*k-s*R)*N,e[2]=(v*U-f*E+h*T)*N,e[3]=(u*E-m*U-g*T)*N,e[4]=(l*y-o*k-c*S)*N,e[5]=(t*k-r*y+s*S)*N,e[6]=(f*w-_*U-h*P)*N,e[7]=(d*U-u*w+g*P)*N,e[8]=(o*I-a*y+c*p)*N,e[9]=(i*y-t*I-s*p)*N,e[10]=(_*E-v*w+h*M)*N,e[11]=(m*w-d*E-g*M)*N,e[12]=(a*S-o*R-l*p)*N,e[13]=(t*R-i*S+r*p)*N,e[14]=(v*P-_*T-f*M)*N,e[15]=(d*T-m*P+u*M)*N,this}scale(e){let t=this.elements,i=e.x,r=e.y,s=e.z;return t[0]*=i,t[4]*=r,t[8]*=s,t[1]*=i,t[5]*=r,t[9]*=s,t[2]*=i,t[6]*=r,t[10]*=s,t[3]*=i,t[7]*=r,t[11]*=s,this}getMaxScaleOnAxis(){let e=this.elements,t=e[0]*e[0]+e[1]*e[1]+e[2]*e[2],i=e[4]*e[4]+e[5]*e[5]+e[6]*e[6],r=e[8]*e[8]+e[9]*e[9]+e[10]*e[10];return Math.sqrt(Math.max(t,i,r))}makeTranslation(e,t,i){return e.isVector3?this.set(1,0,0,e.x,0,1,0,e.y,0,0,1,e.z,0,0,0,1):this.set(1,0,0,e,0,1,0,t,0,0,1,i,0,0,0,1),this}makeRotationX(e){let t=Math.cos(e),i=Math.sin(e);return this.set(1,0,0,0,0,t,-i,0,0,i,t,0,0,0,0,1),this}makeRotationY(e){let t=Math.cos(e),i=Math.sin(e);return this.set(t,0,i,0,0,1,0,0,-i,0,t,0,0,0,0,1),this}makeRotationZ(e){let t=Math.cos(e),i=Math.sin(e);return this.set(t,-i,0,0,i,t,0,0,0,0,1,0,0,0,0,1),this}makeRotationAxis(e,t){let i=Math.cos(t),r=Math.sin(t),s=1-i,o=e.x,a=e.y,l=e.z,c=s*o,d=s*a;return this.set(c*o+i,c*a-r*l,c*l+r*a,0,c*a+r*l,d*a+i,d*l-r*o,0,c*l-r*a,d*l+r*o,s*l*l+i,0,0,0,0,1),this}makeScale(e,t,i){return this.set(e,0,0,0,0,t,0,0,0,0,i,0,0,0,0,1),this}makeShear(e,t,i,r,s,o){return this.set(1,i,s,0,e,1,o,0,t,r,1,0,0,0,0,1),this}compose(e,t,i){let r=this.elements,s=t._x,o=t._y,a=t._z,l=t._w,c=s+s,d=o+o,m=a+a,u=s*c,g=s*d,_=s*m,v=o*d,f=o*m,h=a*m,M=l*c,P=l*d,w=l*m,T=i.x,E=i.y,U=i.z;return r[0]=(1-(v+h))*T,r[1]=(g+w)*T,r[2]=(_-P)*T,r[3]=0,r[4]=(g-w)*E,r[5]=(1-(u+h))*E,r[6]=(f+M)*E,r[7]=0,r[8]=(_+P)*U,r[9]=(f-M)*U,r[10]=(1-(u+v))*U,r[11]=0,r[12]=e.x,r[13]=e.y,r[14]=e.z,r[15]=1,this}decompose(e,t,i){let r=this.elements;e.x=r[12],e.y=r[13],e.z=r[14];let s=this.determinantAffine();if(s===0)return i.set(1,1,1),t.identity(),this;let o=Pr.set(r[0],r[1],r[2]).length(),a=Pr.set(r[4],r[5],r[6]).length(),l=Pr.set(r[8],r[9],r[10]).length();s<0&&(o=-o),Jn.copy(this);let c=1/o,d=1/a,m=1/l;return Jn.elements[0]*=c,Jn.elements[1]*=c,Jn.elements[2]*=c,Jn.elements[4]*=d,Jn.elements[5]*=d,Jn.elements[6]*=d,Jn.elements[8]*=m,Jn.elements[9]*=m,Jn.elements[10]*=m,t.setFromRotationMatrix(Jn),i.x=o,i.y=a,i.z=l,this}makePerspective(e,t,i,r,s,o,a=ei,l=!1){let c=this.elements,d=2*s/(t-e),m=2*s/(i-r),u=(t+e)/(t-e),g=(i+r)/(i-r),_,v;if(l)_=s/(o-s),v=o*s/(o-s);else if(a===ei)_=-(o+s)/(o-s),v=-2*o*s/(o-s);else if(a===qr)_=-o/(o-s),v=-o*s/(o-s);else throw new Error("THREE.Matrix4.makePerspective(): Invalid coordinate system: "+a);return c[0]=d,c[4]=0,c[8]=u,c[12]=0,c[1]=0,c[5]=m,c[9]=g,c[13]=0,c[2]=0,c[6]=0,c[10]=_,c[14]=v,c[3]=0,c[7]=0,c[11]=-1,c[15]=0,this}makeOrthographic(e,t,i,r,s,o,a=ei,l=!1){let c=this.elements,d=2/(t-e),m=2/(i-r),u=-(t+e)/(t-e),g=-(i+r)/(i-r),_,v;if(l)_=1/(o-s),v=o/(o-s);else if(a===ei)_=-2/(o-s),v=-(o+s)/(o-s);else if(a===qr)_=-1/(o-s),v=-s/(o-s);else throw new Error("THREE.Matrix4.makeOrthographic(): Invalid coordinate system: "+a);return c[0]=d,c[4]=0,c[8]=0,c[12]=u,c[1]=0,c[5]=m,c[9]=0,c[13]=g,c[2]=0,c[6]=0,c[10]=_,c[14]=v,c[3]=0,c[7]=0,c[11]=0,c[15]=1,this}equals(e){let t=this.elements,i=e.elements;for(let r=0;r<16;r++)if(t[r]!==i[r])return!1;return!0}fromArray(e,t=0){for(let i=0;i<16;i++)this.elements[i]=e[i+t];return this}toArray(e=[],t=0){let i=this.elements;return e[t]=i[0],e[t+1]=i[1],e[t+2]=i[2],e[t+3]=i[3],e[t+4]=i[4],e[t+5]=i[5],e[t+6]=i[6],e[t+7]=i[7],e[t+8]=i[8],e[t+9]=i[9],e[t+10]=i[10],e[t+11]=i[11],e[t+12]=i[12],e[t+13]=i[13],e[t+14]=i[14],e[t+15]=i[15],e}},Pr=new G,Jn=new wt,Fp=new G(0,0,0),Op=new G(1,1,1),Di=new G,Po=new G,Bn=new G,ah=new wt,lh=new an,zi=class n{constructor(e=0,t=0,i=0,r=n.DEFAULT_ORDER){this.isEuler=!0,this._x=e,this._y=t,this._z=i,this._order=r}get x(){return this._x}set x(e){this._x=e,this._onChangeCallback()}get y(){return this._y}set y(e){this._y=e,this._onChangeCallback()}get z(){return this._z}set z(e){this._z=e,this._onChangeCallback()}get order(){return this._order}set order(e){this._order=e,this._onChangeCallback()}set(e,t,i,r=this._order){return this._x=e,this._y=t,this._z=i,this._order=r,this._onChangeCallback(),this}clone(){return new this.constructor(this._x,this._y,this._z,this._order)}copy(e){return this._x=e._x,this._y=e._y,this._z=e._z,this._order=e._order,this._onChangeCallback(),this}setFromRotationMatrix(e,t=this._order,i=!0){let r=e.elements,s=r[0],o=r[4],a=r[8],l=r[1],c=r[5],d=r[9],m=r[2],u=r[6],g=r[10];switch(t){case"XYZ":this._y=Math.asin(at(a,-1,1)),Math.abs(a)<.9999999?(this._x=Math.atan2(-d,g),this._z=Math.atan2(-o,s)):(this._x=Math.atan2(u,c),this._z=0);break;case"YXZ":this._x=Math.asin(-at(d,-1,1)),Math.abs(d)<.9999999?(this._y=Math.atan2(a,g),this._z=Math.atan2(l,c)):(this._y=Math.atan2(-m,s),this._z=0);break;case"ZXY":this._x=Math.asin(at(u,-1,1)),Math.abs(u)<.9999999?(this._y=Math.atan2(-m,g),this._z=Math.atan2(-o,c)):(this._y=0,this._z=Math.atan2(l,s));break;case"ZYX":this._y=Math.asin(-at(m,-1,1)),Math.abs(m)<.9999999?(this._x=Math.atan2(u,g),this._z=Math.atan2(l,s)):(this._x=0,this._z=Math.atan2(-o,c));break;case"YZX":this._z=Math.asin(at(l,-1,1)),Math.abs(l)<.9999999?(this._x=Math.atan2(-d,c),this._y=Math.atan2(-m,s)):(this._x=0,this._y=Math.atan2(a,g));break;case"XZY":this._z=Math.asin(-at(o,-1,1)),Math.abs(o)<.9999999?(this._x=Math.atan2(u,c),this._y=Math.atan2(a,s)):(this._x=Math.atan2(-d,g),this._y=0);break;default:$e("Euler: .setFromRotationMatrix() encountered an unknown order: "+t)}return this._order=t,i===!0&&this._onChangeCallback(),this}setFromQuaternion(e,t,i){return ah.makeRotationFromQuaternion(e),this.setFromRotationMatrix(ah,t,i)}setFromVector3(e,t=this._order){return this.set(e.x,e.y,e.z,t)}reorder(e){return lh.setFromEuler(this),this.setFromQuaternion(lh,e)}equals(e){return e._x===this._x&&e._y===this._y&&e._z===this._z&&e._order===this._order}fromArray(e){return this._x=e[0],this._y=e[1],this._z=e[2],e[3]!==void 0&&(this._order=e[3]),this._onChangeCallback(),this}toArray(e=[],t=0){return e[t]=this._x,e[t+1]=this._y,e[t+2]=this._z,e[t+3]=this._order,e}_onChange(e){return this._onChangeCallback=e,this}_onChangeCallback(){}*[Symbol.iterator](){yield this._x,yield this._y,yield this._z,yield this._order}};zi.DEFAULT_ORDER="XYZ";var Kr=class{constructor(){this.mask=1}set(e){this.mask=(1<<e|0)>>>0}enable(e){this.mask|=1<<e|0}enableAll(){this.mask=-1}toggle(e){this.mask^=1<<e|0}disable(e){this.mask&=~(1<<e|0)}disableAll(){this.mask=0}test(e){return(this.mask&e.mask)!==0}isEnabled(e){return(this.mask&(1<<e|0))!==0}},Bp=0,ch=new G,Ir=new an,_i=new wt,Io=new G,bs=new G,zp=new G,Hp=new an,uh=new G(1,0,0),hh=new G(0,1,0),dh=new G(0,0,1),fh={type:"added"},Gp={type:"removed"},Lr={type:"childadded",child:null},ec={type:"childremoved",child:null},vn=class n extends ti{constructor(){super(),this.isObject3D=!0,Object.defineProperty(this,"id",{value:Bp++}),this.uuid=cs(),this.name="",this.type="Object3D",this.parent=null,this.children=[],this.up=n.DEFAULT_UP.clone();let e=new G,t=new zi,i=new an,r=new G(1,1,1);function s(){i.setFromEuler(t,!1)}function o(){t.setFromQuaternion(i,void 0,!1)}t._onChange(s),i._onChange(o),Object.defineProperties(this,{position:{configurable:!0,enumerable:!0,value:e},rotation:{configurable:!0,enumerable:!0,value:t},quaternion:{configurable:!0,enumerable:!0,value:i},scale:{configurable:!0,enumerable:!0,value:r},modelViewMatrix:{value:new wt},normalMatrix:{value:new je}}),this.matrix=new wt,this.matrixWorld=new wt,this.matrixAutoUpdate=n.DEFAULT_MATRIX_AUTO_UPDATE,this.matrixWorldAutoUpdate=n.DEFAULT_MATRIX_WORLD_AUTO_UPDATE,this.matrixWorldNeedsUpdate=!1,this.layers=new Kr,this.visible=!0,this.castShadow=!1,this.receiveShadow=!1,this.frustumCulled=!0,this.renderOrder=0,this.animations=[],this.customDepthMaterial=void 0,this.customDistanceMaterial=void 0,this.static=!1,this.userData={},this.pivot=null}onBeforeShadow(){}onAfterShadow(){}onBeforeRender(){}onAfterRender(){}applyMatrix4(e){this.matrixAutoUpdate&&this.updateMatrix(),this.matrix.premultiply(e),this.matrix.decompose(this.position,this.quaternion,this.scale)}applyQuaternion(e){return this.quaternion.premultiply(e),this}setRotationFromAxisAngle(e,t){this.quaternion.setFromAxisAngle(e,t)}setRotationFromEuler(e){this.quaternion.setFromEuler(e,!0)}setRotationFromMatrix(e){this.quaternion.setFromRotationMatrix(e)}setRotationFromQuaternion(e){this.quaternion.copy(e)}rotateOnAxis(e,t){return Ir.setFromAxisAngle(e,t),this.quaternion.multiply(Ir),this}rotateOnWorldAxis(e,t){return Ir.setFromAxisAngle(e,t),this.quaternion.premultiply(Ir),this}rotateX(e){return this.rotateOnAxis(uh,e)}rotateY(e){return this.rotateOnAxis(hh,e)}rotateZ(e){return this.rotateOnAxis(dh,e)}translateOnAxis(e,t){return ch.copy(e).applyQuaternion(this.quaternion),this.position.add(ch.multiplyScalar(t)),this}translateX(e){return this.translateOnAxis(uh,e)}translateY(e){return this.translateOnAxis(hh,e)}translateZ(e){return this.translateOnAxis(dh,e)}localToWorld(e){return this.updateWorldMatrix(!0,!1),e.applyMatrix4(this.matrixWorld)}worldToLocal(e){return this.updateWorldMatrix(!0,!1),e.applyMatrix4(_i.copy(this.matrixWorld).invert())}lookAt(e,t,i){e.isVector3?Io.copy(e):Io.set(e,t,i);let r=this.parent;this.updateWorldMatrix(!0,!1),bs.setFromMatrixPosition(this.matrixWorld),this.isCamera||this.isLight?_i.lookAt(bs,Io,this.up):_i.lookAt(Io,bs,this.up),this.quaternion.setFromRotationMatrix(_i),r&&(_i.extractRotation(r.matrixWorld),Ir.setFromRotationMatrix(_i),this.quaternion.premultiply(Ir.invert()))}add(e){if(arguments.length>1){for(let t=0;t<arguments.length;t++)this.add(arguments[t]);return this}return e===this?(Ze("Object3D.add: object can't be added as a child of itself.",e),this):(e&&e.isObject3D?(e.removeFromParent(),e.parent=this,this.children.push(e),e.dispatchEvent(fh),Lr.child=e,this.dispatchEvent(Lr),Lr.child=null):Ze("Object3D.add: object not an instance of THREE.Object3D.",e),this)}remove(e){if(arguments.length>1){for(let i=0;i<arguments.length;i++)this.remove(arguments[i]);return this}let t=this.children.indexOf(e);return t!==-1&&(e.parent=null,this.children.splice(t,1),e.dispatchEvent(Gp),ec.child=e,this.dispatchEvent(ec),ec.child=null),this}removeFromParent(){let e=this.parent;return e!==null&&e.remove(this),this}clear(){return this.remove(...this.children)}attach(e){return this.updateWorldMatrix(!0,!1),_i.copy(this.matrixWorld).invert(),e.parent!==null&&(e.parent.updateWorldMatrix(!0,!1),_i.multiply(e.parent.matrixWorld)),e.applyMatrix4(_i),e.removeFromParent(),e.parent=this,this.children.push(e),e.updateWorldMatrix(!1,!0),e.dispatchEvent(fh),Lr.child=e,this.dispatchEvent(Lr),Lr.child=null,this}getObjectById(e){return this.getObjectByProperty("id",e)}getObjectByName(e){return this.getObjectByProperty("name",e)}getObjectByProperty(e,t){if(this[e]===t)return this;for(let i=0,r=this.children.length;i<r;i++){let o=this.children[i].getObjectByProperty(e,t);if(o!==void 0)return o}}getObjectsByProperty(e,t,i=[]){this[e]===t&&i.push(this);let r=this.children;for(let s=0,o=r.length;s<o;s++)r[s].getObjectsByProperty(e,t,i);return i}getWorldPosition(e){return this.updateWorldMatrix(!0,!1),e.setFromMatrixPosition(this.matrixWorld)}getWorldQuaternion(e){return this.updateWorldMatrix(!0,!1),this.matrixWorld.decompose(bs,e,zp),e}getWorldScale(e){return this.updateWorldMatrix(!0,!1),this.matrixWorld.decompose(bs,Hp,e),e}getWorldDirection(e){this.updateWorldMatrix(!0,!1);let t=this.matrixWorld.elements;return e.set(t[8],t[9],t[10]).normalize()}raycast(){}intersectsFrustum(){}traverse(e){e(this);let t=this.children;for(let i=0,r=t.length;i<r;i++)t[i].traverse(e)}traverseVisible(e){if(this.visible===!1)return;e(this);let t=this.children;for(let i=0,r=t.length;i<r;i++)t[i].traverseVisible(e)}traverseAncestors(e){let t=this.parent;t!==null&&(e(t),t.traverseAncestors(e))}updateMatrix(){this.matrix.compose(this.position,this.quaternion,this.scale);let e=this.pivot;if(e!==null){let t=e.x,i=e.y,r=e.z,s=this.matrix.elements;s[12]+=t-s[0]*t-s[4]*i-s[8]*r,s[13]+=i-s[1]*t-s[5]*i-s[9]*r,s[14]+=r-s[2]*t-s[6]*i-s[10]*r}this.matrixWorldNeedsUpdate=!0}updateMatrixWorld(e){this.matrixAutoUpdate&&this.updateMatrix(),(this.matrixWorldNeedsUpdate||e)&&(this.matrixWorldAutoUpdate===!0&&(this.parent===null?this.matrixWorld.copy(this.matrix):this.matrixWorld.multiplyMatrices(this.parent.matrixWorld,this.matrix)),this.matrixWorldNeedsUpdate=!1,e=!0);let t=this.children;for(let i=0,r=t.length;i<r;i++)t[i].updateMatrixWorld(e)}updateWorldMatrix(e,t,i=!1){let r=this.parent;if(e===!0&&r!==null&&r.updateWorldMatrix(!0,!1),this.matrixAutoUpdate&&this.updateMatrix(),(this.matrixWorldNeedsUpdate||i)&&(this.matrixWorldAutoUpdate===!0&&(this.parent===null?this.matrixWorld.copy(this.matrix):this.matrixWorld.multiplyMatrices(this.parent.matrixWorld,this.matrix)),this.matrixWorldNeedsUpdate=!1,i=!0),t===!0){let s=this.children;for(let o=0,a=s.length;o<a;o++)s[o].updateWorldMatrix(!1,!0,i)}}toJSON(e){let t=e===void 0||typeof e=="string",i={};t&&(e={geometries:{},materials:{},textures:{},images:{},shapes:{},skeletons:{},animations:{},nodes:{}},i.metadata={version:4.7,type:"Object",generator:"Object3D.toJSON"});let r={};r.uuid=this.uuid,r.type=this.type,r.name=this.name,r.castShadow=this.castShadow,r.receiveShadow=this.receiveShadow,r.visible=this.visible,r.frustumCulled=this.frustumCulled,r.renderOrder=this.renderOrder,r.static=this.static,r.matrixAutoUpdate=this.matrixAutoUpdate,Object.keys(this.userData).length>0&&(r.userData=this.userData),r.layers=this.layers.mask,r.matrix=this.matrix.toArray(),r.up=this.up.toArray(),this.pivot!==null&&(r.pivot=this.pivot.toArray()),this.morphTargetDictionary!==void 0&&(r.morphTargetDictionary=Object.assign({},this.morphTargetDictionary)),this.morphTargetInfluences!==void 0&&(r.morphTargetInfluences=this.morphTargetInfluences.slice()),this.isInstancedMesh&&(r.type="InstancedMesh",r.count=this.count,r.instanceMatrix=this.instanceMatrix.toJSON(),this.instanceColor!==null&&(r.instanceColor=this.instanceColor.toJSON())),this.isBatchedMesh&&(r.type="BatchedMesh",r.perObjectFrustumCulled=this.perObjectFrustumCulled,r.sortObjects=this.sortObjects,r.drawRanges=this._drawRanges,r.reservedRanges=this._reservedRanges,r.geometryInfo=this._geometryInfo.map(a=>({...a,boundingBox:a.boundingBox?a.boundingBox.toJSON():void 0,boundingSphere:a.boundingSphere?a.boundingSphere.toJSON():void 0})),r.instanceInfo=this._instanceInfo.map(a=>({...a})),r.availableInstanceIds=this._availableInstanceIds.slice(),r.availableGeometryIds=this._availableGeometryIds.slice(),r.nextIndexStart=this._nextIndexStart,r.nextVertexStart=this._nextVertexStart,r.geometryCount=this._geometryCount,r.maxInstanceCount=this._maxInstanceCount,r.maxVertexCount=this._maxVertexCount,r.maxIndexCount=this._maxIndexCount,r.geometryInitialized=this._geometryInitialized,r.matricesTexture=this._matricesTexture.toJSON(e),r.indirectTexture=this._indirectTexture.toJSON(e),this._colorsTexture!==null&&(r.colorsTexture=this._colorsTexture.toJSON(e)),this.boundingSphere!==null&&(r.boundingSphere=this.boundingSphere.toJSON()),this.boundingBox!==null&&(r.boundingBox=this.boundingBox.toJSON()));function s(a,l){return a[l.uuid]===void 0&&(a[l.uuid]=l.toJSON(e)),l.uuid}if(this.isScene)this.background&&(this.background.isColor?r.background=this.background.toJSON():this.background.isTexture&&(r.background=this.background.toJSON(e).uuid)),this.environment&&this.environment.isTexture&&this.environment.isRenderTargetTexture!==!0&&(r.environment=this.environment.toJSON(e).uuid);else if(this.isMesh||this.isLine||this.isPoints){r.geometry=s(e.geometries,this.geometry);let a=this.geometry.parameters;if(a!==void 0&&a.shapes!==void 0){let l=a.shapes;if(Array.isArray(l))for(let c=0,d=l.length;c<d;c++){let m=l[c];s(e.shapes,m)}else s(e.shapes,l)}}if(this.isSkinnedMesh&&(r.bindMode=this.bindMode,r.bindMatrix=this.bindMatrix.toArray(),this.skeleton!==void 0&&(s(e.skeletons,this.skeleton),r.skeleton=this.skeleton.uuid)),this.material!==void 0)if(Array.isArray(this.material)){let a=[];for(let l=0,c=this.material.length;l<c;l++)a.push(s(e.materials,this.material[l]));r.material=a}else r.material=s(e.materials,this.material);if(this.children.length>0){r.children=[];for(let a=0;a<this.children.length;a++)r.children.push(this.children[a].toJSON(e).object)}if(this.animations.length>0){r.animations=[];for(let a=0;a<this.animations.length;a++){let l=this.animations[a];r.animations.push(s(e.animations,l))}}if(t){let a=o(e.geometries),l=o(e.materials),c=o(e.textures),d=o(e.images),m=o(e.shapes),u=o(e.skeletons),g=o(e.animations),_=o(e.nodes);a.length>0&&(i.geometries=a),l.length>0&&(i.materials=l),c.length>0&&(i.textures=c),d.length>0&&(i.images=d),m.length>0&&(i.shapes=m),u.length>0&&(i.skeletons=u),g.length>0&&(i.animations=g),_.length>0&&(i.nodes=_)}return i.object=r,i;function o(a){let l=[];for(let c in a){let d=a[c];delete d.metadata,l.push(d)}return l}}clone(e){return new this.constructor().copy(this,e)}copy(e,t=!0){if(this.name=e.name,this.up.copy(e.up),this.position.copy(e.position),this.rotation.order=e.rotation.order,this.quaternion.copy(e.quaternion),this.scale.copy(e.scale),this.pivot=e.pivot!==null?e.pivot.clone():null,this.matrix.copy(e.matrix),this.matrixWorld.copy(e.matrixWorld),this.matrixAutoUpdate=e.matrixAutoUpdate,this.matrixWorldAutoUpdate=e.matrixWorldAutoUpdate,this.matrixWorldNeedsUpdate=e.matrixWorldNeedsUpdate,this.layers.mask=e.layers.mask,this.visible=e.visible,this.castShadow=e.castShadow,this.receiveShadow=e.receiveShadow,this.frustumCulled=e.frustumCulled,this.renderOrder=e.renderOrder,this.static=e.static,this.animations=e.animations.slice(),this.userData=JSON.parse(JSON.stringify(e.userData)),t===!0)for(let i=0;i<e.children.length;i++){let r=e.children[i];this.add(r.clone())}return this}dispose(){this.dispatchEvent({type:"dispose"})}};vn.DEFAULT_UP=new G(0,1,0);vn.DEFAULT_MATRIX_AUTO_UPDATE=!0;vn.DEFAULT_MATRIX_WORLD_AUTO_UPDATE=!0;var ht=class extends vn{constructor(){super(),this.isGroup=!0,this.type="Group"}},Vp={type:"move"},Jr=class{constructor(){this._targetRay=null,this._grip=null,this._hand=null}getHandSpace(){return this._hand===null&&(this._hand=new ht,this._hand.matrixAutoUpdate=!1,this._hand.visible=!1,this._hand.joints={},this._hand.inputState={pinching:!1}),this._hand}getTargetRaySpace(){return this._targetRay===null&&(this._targetRay=new ht,this._targetRay.matrixAutoUpdate=!1,this._targetRay.visible=!1,this._targetRay.hasLinearVelocity=!1,this._targetRay.linearVelocity=new G,this._targetRay.hasAngularVelocity=!1,this._targetRay.angularVelocity=new G),this._targetRay}getGripSpace(){return this._grip===null&&(this._grip=new ht,this._grip.matrixAutoUpdate=!1,this._grip.visible=!1,this._grip.hasLinearVelocity=!1,this._grip.linearVelocity=new G,this._grip.hasAngularVelocity=!1,this._grip.angularVelocity=new G,this._grip.eventsEnabled=!1),this._grip}dispatchEvent(e){return this._targetRay!==null&&this._targetRay.dispatchEvent(e),this._grip!==null&&this._grip.dispatchEvent(e),this._hand!==null&&this._hand.dispatchEvent(e),this}connect(e){if(e&&e.hand){let t=this._hand;if(t)for(let i of e.hand.values())this._getHandJoint(t,i)}return this.dispatchEvent({type:"connected",data:e}),this}disconnect(e){return this.dispatchEvent({type:"disconnected",data:e}),this._targetRay!==null&&(this._targetRay.visible=!1),this._grip!==null&&(this._grip.visible=!1),this._hand!==null&&(this._hand.visible=!1),this}update(e,t,i){let r=null,s=null,o=null,a=this._targetRay,l=this._grip,c=this._hand;if(e&&t.session.visibilityState!=="visible-blurred"){if(c&&e.hand){o=!0;for(let v of e.hand.values()){let f=t.getJointPose(v,i),h=this._getHandJoint(c,v);f!==null&&(h.matrix.fromArray(f.transform.matrix),h.matrix.decompose(h.position,h.rotation,h.scale),h.matrixWorldNeedsUpdate=!0,h.jointRadius=f.radius),h.visible=f!==null}let d=c.joints["index-finger-tip"],m=c.joints["thumb-tip"],u=d.position.distanceTo(m.position),g=.02,_=.005;c.inputState.pinching&&u>g+_?(c.inputState.pinching=!1,this.dispatchEvent({type:"pinchend",handedness:e.handedness,target:this})):!c.inputState.pinching&&u<=g-_&&(c.inputState.pinching=!0,this.dispatchEvent({type:"pinchstart",handedness:e.handedness,target:this}))}else l!==null&&e.gripSpace&&(s=t.getPose(e.gripSpace,i),s!==null&&(l.matrix.fromArray(s.transform.matrix),l.matrix.decompose(l.position,l.rotation,l.scale),l.matrixWorldNeedsUpdate=!0,s.linearVelocity?(l.hasLinearVelocity=!0,l.linearVelocity.copy(s.linearVelocity)):l.hasLinearVelocity=!1,s.angularVelocity?(l.hasAngularVelocity=!0,l.angularVelocity.copy(s.angularVelocity)):l.hasAngularVelocity=!1,l.eventsEnabled&&l.dispatchEvent({type:"gripUpdated",data:e,target:this})));a!==null&&(r=t.getPose(e.targetRaySpace,i),r===null&&s!==null&&(r=s),r!==null&&(a.matrix.fromArray(r.transform.matrix),a.matrix.decompose(a.position,a.rotation,a.scale),a.matrixWorldNeedsUpdate=!0,r.linearVelocity?(a.hasLinearVelocity=!0,a.linearVelocity.copy(r.linearVelocity)):a.hasLinearVelocity=!1,r.angularVelocity?(a.hasAngularVelocity=!0,a.angularVelocity.copy(r.angularVelocity)):a.hasAngularVelocity=!1,this.dispatchEvent(Vp)))}return a!==null&&(a.visible=r!==null),l!==null&&(l.visible=s!==null),c!==null&&(c.visible=o!==null),this}_getHandJoint(e,t){if(e.joints[t.jointName]===void 0){let i=new ht;i.matrixAutoUpdate=!1,i.visible=!1,e.joints[t.jointName]=i,e.add(i)}return e.joints[t.jointName]}},pd={aliceblue:15792383,antiquewhite:16444375,aqua:65535,aquamarine:8388564,azure:15794175,beige:16119260,bisque:16770244,black:0,blanchedalmond:16772045,blue:255,blueviolet:9055202,brown:10824234,burlywood:14596231,cadetblue:6266528,chartreuse:8388352,chocolate:13789470,coral:16744272,cornflowerblue:6591981,cornsilk:16775388,crimson:14423100,cyan:65535,darkblue:139,darkcyan:35723,darkgoldenrod:12092939,darkgray:11119017,darkgreen:25600,darkgrey:11119017,darkkhaki:12433259,darkmagenta:9109643,darkolivegreen:5597999,darkorange:16747520,darkorchid:10040012,darkred:9109504,darksalmon:15308410,darkseagreen:9419919,darkslateblue:4734347,darkslategray:3100495,darkslategrey:3100495,darkturquoise:52945,darkviolet:9699539,deeppink:16716947,deepskyblue:49151,dimgray:6908265,dimgrey:6908265,dodgerblue:2003199,firebrick:11674146,floralwhite:16775920,forestgreen:2263842,fuchsia:16711935,gainsboro:14474460,ghostwhite:16316671,gold:16766720,goldenrod:14329120,gray:8421504,green:32768,greenyellow:11403055,grey:8421504,honeydew:15794160,hotpink:16738740,indianred:13458524,indigo:4915330,ivory:16777200,khaki:15787660,lavender:15132410,lavenderblush:16773365,lawngreen:8190976,lemonchiffon:16775885,lightblue:11393254,lightcoral:15761536,lightcyan:14745599,lightgoldenrodyellow:16448210,lightgray:13882323,lightgreen:9498256,lightgrey:13882323,lightpink:16758465,lightsalmon:16752762,lightseagreen:2142890,lightskyblue:8900346,lightslategray:7833753,lightslategrey:7833753,lightsteelblue:11584734,lightyellow:16777184,lime:65280,limegreen:3329330,linen:16445670,magenta:16711935,maroon:8388608,mediumaquamarine:6737322,mediumblue:205,mediumorchid:12211667,mediumpurple:9662683,mediumseagreen:3978097,mediumslateblue:8087790,mediumspringgreen:64154,mediumturquoise:4772300,mediumvioletred:13047173,midnightblue:1644912,mintcream:16121850,mistyrose:16770273,moccasin:16770229,navajowhite:16768685,navy:128,oldlace:16643558,olive:8421376,olivedrab:7048739,orange:16753920,orangered:16729344,orchid:14315734,palegoldenrod:15657130,palegreen:10025880,paleturquoise:11529966,palevioletred:14381203,papayawhip:16773077,peachpuff:16767673,peru:13468991,pink:16761035,plum:14524637,powderblue:11591910,purple:8388736,rebeccapurple:6697881,red:16711680,rosybrown:12357519,royalblue:4286945,saddlebrown:9127187,salmon:16416882,sandybrown:16032864,seagreen:3050327,seashell:16774638,sienna:10506797,silver:12632256,skyblue:8900331,slateblue:6970061,slategray:7372944,slategrey:7372944,snow:16775930,springgreen:65407,steelblue:4620980,tan:13808780,teal:32896,thistle:14204888,tomato:16737095,turquoise:4251856,violet:15631086,wheat:16113331,white:16777215,whitesmoke:16119285,yellow:16776960,yellowgreen:10145074},Ni={h:0,s:0,l:0},Lo={h:0,s:0,l:0};function tc(n,e,t){return t<0&&(t+=1),t>1&&(t-=1),t<1/6?n+(e-n)*6*t:t<1/2?e:t<2/3?n+(e-n)*6*(2/3-t):n}var Ne=class{constructor(e,t,i){return this.isColor=!0,this.r=1,this.g=1,this.b=1,this.set(e,t,i)}set(e,t,i){if(t===void 0&&i===void 0){let r=e;r&&r.isColor?this.copy(r):typeof r=="number"?this.setHex(r):typeof r=="string"&&this.setStyle(r)}else this.setRGB(e,t,i);return this}setScalar(e){return this.r=e,this.g=e,this.b=e,this}setHex(e,t=kt){return e=Math.floor(e),this.r=(e>>16&255)/255,this.g=(e>>8&255)/255,this.b=(e&255)/255,ut.colorSpaceToWorking(this,t),this}setRGB(e,t,i,r=ut.workingColorSpace){return this.r=e,this.g=t,this.b=i,ut.colorSpaceToWorking(this,r),this}setHSL(e,t,i,r=ut.workingColorSpace){if(e=Zc(e,1),t=at(t,0,1),i=at(i,0,1),t===0)this.r=this.g=this.b=i;else{let s=i<=.5?i*(1+t):i+t-i*t,o=2*i-s;this.r=tc(o,s,e+1/3),this.g=tc(o,s,e),this.b=tc(o,s,e-1/3)}return ut.colorSpaceToWorking(this,r),this}setStyle(e,t=kt){function i(s){s!==void 0&&parseFloat(s)<1&&$e("Color: Alpha component of "+e+" will be ignored.")}let r;if(r=/^(\w+)\(([^\)]*)\)/.exec(e)){let s,o=r[1],a=r[2];switch(o){case"rgb":case"rgba":if(s=/^\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*(\d*\.?\d+)\s*)?$/.exec(a))return i(s[4]),this.setRGB(Math.min(255,parseInt(s[1],10))/255,Math.min(255,parseInt(s[2],10))/255,Math.min(255,parseInt(s[3],10))/255,t);if(s=/^\s*(\d+)\%\s*,\s*(\d+)\%\s*,\s*(\d+)\%\s*(?:,\s*(\d*\.?\d+)\s*)?$/.exec(a))return i(s[4]),this.setRGB(Math.min(100,parseInt(s[1],10))/100,Math.min(100,parseInt(s[2],10))/100,Math.min(100,parseInt(s[3],10))/100,t);break;case"hsl":case"hsla":if(s=/^\s*(\d*\.?\d+)\s*,\s*(\d*\.?\d+)\%\s*,\s*(\d*\.?\d+)\%\s*(?:,\s*(\d*\.?\d+)\s*)?$/.exec(a))return i(s[4]),this.setHSL(parseFloat(s[1])/360,parseFloat(s[2])/100,parseFloat(s[3])/100,t);break;default:$e("Color: Unknown color model "+e)}}else if(r=/^\#([A-Fa-f\d]+)$/.exec(e)){let s=r[1],o=s.length;if(o===3)return this.setRGB(parseInt(s.charAt(0),16)/15,parseInt(s.charAt(1),16)/15,parseInt(s.charAt(2),16)/15,t);if(o===6)return this.setHex(parseInt(s,16),t);$e("Color: Invalid hex color "+e)}else if(e&&e.length>0)return this.setColorName(e,t);return this}setColorName(e,t=kt){let i=pd[e.toLowerCase()];return i!==void 0?this.setHex(i,t):$e("Color: Unknown color "+e),this}clone(){return new this.constructor(this.r,this.g,this.b)}copy(e){return this.r=e.r,this.g=e.g,this.b=e.b,this}copySRGBToLinear(e){return this.r=wi(e.r),this.g=wi(e.g),this.b=wi(e.b),this}copyLinearToSRGB(e){return this.r=Wr(e.r),this.g=Wr(e.g),this.b=Wr(e.b),this}convertSRGBToLinear(){return this.copySRGBToLinear(this),this}convertLinearToSRGB(){return this.copyLinearToSRGB(this),this}getHex(e=kt){return ut.workingToColorSpace(En.copy(this),e),Math.round(at(En.r*255,0,255))*65536+Math.round(at(En.g*255,0,255))*256+Math.round(at(En.b*255,0,255))}getHexString(e=kt){return("000000"+this.getHex(e).toString(16)).slice(-6)}getHSL(e,t=ut.workingColorSpace){ut.workingToColorSpace(En.copy(this),t);let i=En.r,r=En.g,s=En.b,o=Math.max(i,r,s),a=Math.min(i,r,s),l,c,d=(a+o)/2;if(a===o)l=0,c=0;else{let m=o-a;switch(c=d<=.5?m/(o+a):m/(2-o-a),o){case i:l=(r-s)/m+(r<s?6:0);break;case r:l=(s-i)/m+2;break;case s:l=(i-r)/m+4;break}l/=6}return e.h=l,e.s=c,e.l=d,e}getRGB(e,t=ut.workingColorSpace){return ut.workingToColorSpace(En.copy(this),t),e.r=En.r,e.g=En.g,e.b=En.b,e}getStyle(e=kt){ut.workingToColorSpace(En.copy(this),e);let t=En.r,i=En.g,r=En.b;return e!==kt?`color(${e} ${t.toFixed(3)} ${i.toFixed(3)} ${r.toFixed(3)})`:`rgb(${Math.round(t*255)},${Math.round(i*255)},${Math.round(r*255)})`}offsetHSL(e,t,i){return this.getHSL(Ni),this.setHSL(Ni.h+e,Ni.s+t,Ni.l+i)}add(e){return this.r+=e.r,this.g+=e.g,this.b+=e.b,this}addColors(e,t){return this.r=e.r+t.r,this.g=e.g+t.g,this.b=e.b+t.b,this}addScalar(e){return this.r+=e,this.g+=e,this.b+=e,this}sub(e){return this.r=Math.max(0,this.r-e.r),this.g=Math.max(0,this.g-e.g),this.b=Math.max(0,this.b-e.b),this}multiply(e){return this.r*=e.r,this.g*=e.g,this.b*=e.b,this}multiplyScalar(e){return this.r*=e,this.g*=e,this.b*=e,this}lerp(e,t){return this.r+=(e.r-this.r)*t,this.g+=(e.g-this.g)*t,this.b+=(e.b-this.b)*t,this}lerpColors(e,t,i){return this.r=e.r+(t.r-e.r)*i,this.g=e.g+(t.g-e.g)*i,this.b=e.b+(t.b-e.b)*i,this}lerpHSL(e,t){this.getHSL(Ni),e.getHSL(Lo);let i=Rs(Ni.h,Lo.h,t),r=Rs(Ni.s,Lo.s,t),s=Rs(Ni.l,Lo.l,t);return this.setHSL(i,r,s),this}setFromVector3(e){return this.r=e.x,this.g=e.y,this.b=e.z,this}applyMatrix3(e){let t=this.r,i=this.g,r=this.b,s=e.elements;return this.r=s[0]*t+s[3]*i+s[6]*r,this.g=s[1]*t+s[4]*i+s[7]*r,this.b=s[2]*t+s[5]*i+s[8]*r,this}equals(e){return e.r===this.r&&e.g===this.g&&e.b===this.b}fromArray(e,t=0){return this.r=e[t],this.g=e[t+1],this.b=e[t+2],this}toArray(e=[],t=0){return e[t]=this.r,e[t+1]=this.g,e[t+2]=this.b,e}fromBufferAttribute(e,t){return this.r=e.getX(t),this.g=e.getY(t),this.b=e.getZ(t),this}toJSON(){return this.getHex()}*[Symbol.iterator](){yield this.r,yield this.g,yield this.b}},En=new Ne;Ne.NAMES=pd;var Ds=class n{constructor(e,t=1,i=1e3){this.isFog=!0,this.name="",this.color=new Ne(e),this.near=t,this.far=i}clone(){return new n(this.color,this.near,this.far)}toJSON(){return{type:"Fog",name:this.name,color:this.color.getHex(),near:this.near,far:this.far}}},Ns=class extends vn{constructor(){super(),this.isScene=!0,this.type="Scene",this.background=null,this.environment=null,this.fog=null,this.backgroundBlurriness=0,this.backgroundIntensity=1,this.backgroundRotation=new zi,this.environmentIntensity=1,this.environmentRotation=new zi,this.overrideMaterial=null,typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("observe",{detail:this}))}copy(e,t){return super.copy(e,t),e.background!==null&&(this.background=e.background.clone()),e.environment!==null&&(this.environment=e.environment.clone()),e.fog!==null&&(this.fog=e.fog.clone()),this.backgroundBlurriness=e.backgroundBlurriness,this.backgroundIntensity=e.backgroundIntensity,this.backgroundRotation.copy(e.backgroundRotation),this.environmentIntensity=e.environmentIntensity,this.environmentRotation.copy(e.environmentRotation),e.overrideMaterial!==null&&(this.overrideMaterial=e.overrideMaterial.clone()),this.matrixAutoUpdate=e.matrixAutoUpdate,this}toJSON(e){let t=super.toJSON(e);return this.fog!==null&&(t.object.fog=this.fog.toJSON()),t.object.backgroundBlurriness=this.backgroundBlurriness,t.object.backgroundIntensity=this.backgroundIntensity,t.object.backgroundRotation=this.backgroundRotation.toArray(),t.object.environmentIntensity=this.environmentIntensity,t.object.environmentRotation=this.environmentRotation.toArray(),t}},jn=new G,yi=new G,nc=new G,bi=new G,kr=new G,Dr=new G,ph=new G,ic=new G,rc=new G,sc=new G,oc=new Bt,ac=new Bt,lc=new Bt,Bi=class n{constructor(e=new G,t=new G,i=new G){this.a=e,this.b=t,this.c=i}static getNormal(e,t,i,r){r.subVectors(i,t),jn.subVectors(e,t),r.cross(jn);let s=r.lengthSq();return s>0?r.multiplyScalar(1/Math.sqrt(s)):r.set(0,0,0)}static getBarycoord(e,t,i,r,s){jn.subVectors(r,t),yi.subVectors(i,t),nc.subVectors(e,t);let o=jn.dot(jn),a=jn.dot(yi),l=jn.dot(nc),c=yi.dot(yi),d=yi.dot(nc),m=o*c-a*a;if(m===0)return s.set(0,0,0),null;let u=1/m,g=(c*l-a*d)*u,_=(o*d-a*l)*u;return s.set(1-g-_,_,g)}static containsPoint(e,t,i,r){return this.getBarycoord(e,t,i,r,bi)===null?!1:bi.x>=0&&bi.y>=0&&bi.x+bi.y<=1}static getInterpolation(e,t,i,r,s,o,a,l){return this.getBarycoord(e,t,i,r,bi)===null?(l.x=0,l.y=0,"z"in l&&(l.z=0),"w"in l&&(l.w=0),null):(l.setScalar(0),l.addScaledVector(s,bi.x),l.addScaledVector(o,bi.y),l.addScaledVector(a,bi.z),l)}static getInterpolatedAttribute(e,t,i,r,s,o){return oc.setScalar(0),ac.setScalar(0),lc.setScalar(0),oc.fromBufferAttribute(e,t),ac.fromBufferAttribute(e,i),lc.fromBufferAttribute(e,r),o.setScalar(0),o.addScaledVector(oc,s.x),o.addScaledVector(ac,s.y),o.addScaledVector(lc,s.z),o}static isFrontFacing(e,t,i,r){return jn.subVectors(i,t),yi.subVectors(e,t),jn.cross(yi).dot(r)<0}set(e,t,i){return this.a.copy(e),this.b.copy(t),this.c.copy(i),this}setFromPointsAndIndices(e,t,i,r){return this.a.copy(e[t]),this.b.copy(e[i]),this.c.copy(e[r]),this}setFromAttributeAndIndices(e,t,i,r){return this.a.fromBufferAttribute(e,t),this.b.fromBufferAttribute(e,i),this.c.fromBufferAttribute(e,r),this}clone(){return new this.constructor().copy(this)}copy(e){return this.a.copy(e.a),this.b.copy(e.b),this.c.copy(e.c),this}getArea(){return jn.subVectors(this.c,this.b),yi.subVectors(this.a,this.b),jn.cross(yi).length()*.5}getMidpoint(e){return e.addVectors(this.a,this.b).add(this.c).multiplyScalar(1/3)}getNormal(e){return n.getNormal(this.a,this.b,this.c,e)}getPlane(e){return e.setFromCoplanarPoints(this.a,this.b,this.c)}getBarycoord(e,t){return n.getBarycoord(e,this.a,this.b,this.c,t)}getInterpolation(e,t,i,r,s){return n.getInterpolation(e,this.a,this.b,this.c,t,i,r,s)}containsPoint(e){return n.containsPoint(e,this.a,this.b,this.c)}isFrontFacing(e){return n.isFrontFacing(this.a,this.b,this.c,e)}intersectsBox(e){return e.intersectsTriangle(this)}closestPointToPoint(e,t){let i=this.a,r=this.b,s=this.c,o,a;kr.subVectors(r,i),Dr.subVectors(s,i),ic.subVectors(e,i);let l=kr.dot(ic),c=Dr.dot(ic);if(l<=0&&c<=0)return t.copy(i);rc.subVectors(e,r);let d=kr.dot(rc),m=Dr.dot(rc);if(d>=0&&m<=d)return t.copy(r);let u=l*m-d*c;if(u<=0&&l>=0&&d<=0)return o=l/(l-d),t.copy(i).addScaledVector(kr,o);sc.subVectors(e,s);let g=kr.dot(sc),_=Dr.dot(sc);if(_>=0&&g<=_)return t.copy(s);let v=g*c-l*_;if(v<=0&&c>=0&&_<=0)return a=c/(c-_),t.copy(i).addScaledVector(Dr,a);let f=d*_-g*m;if(f<=0&&m-d>=0&&g-_>=0)return ph.subVectors(s,r),a=(m-d)/(m-d+(g-_)),t.copy(r).addScaledVector(ph,a);let h=1/(f+v+u);return o=v*h,a=u*h,t.copy(i).addScaledVector(kr,o).addScaledVector(Dr,a)}equals(e){return e.a.equals(this.a)&&e.b.equals(this.b)&&e.c.equals(this.c)}},Dn=class{constructor(e=new G(1/0,1/0,1/0),t=new G(-1/0,-1/0,-1/0)){this.isBox3=!0,this.min=e,this.max=t}set(e,t){return this.min.copy(e),this.max.copy(t),this}setFromArray(e){this.makeEmpty();for(let t=0,i=e.length;t<i;t+=3)this.expandByPoint(Qn.fromArray(e,t));return this}setFromBufferAttribute(e){this.makeEmpty();for(let t=0,i=e.count;t<i;t++)this.expandByPoint(Qn.fromBufferAttribute(e,t));return this}setFromPoints(e){this.makeEmpty();for(let t=0,i=e.length;t<i;t++)this.expandByPoint(e[t]);return this}setFromCenterAndSize(e,t){let i=Qn.copy(t).multiplyScalar(.5);return this.min.copy(e).sub(i),this.max.copy(e).add(i),this}setFromObject(e,t=!1){return this.makeEmpty(),this.expandByObject(e,t)}clone(){return new this.constructor().copy(this)}copy(e){return this.min.copy(e.min),this.max.copy(e.max),this}makeEmpty(){return this.min.x=this.min.y=this.min.z=1/0,this.max.x=this.max.y=this.max.z=-1/0,this}isEmpty(){return this.max.x<this.min.x||this.max.y<this.min.y||this.max.z<this.min.z}getCenter(e){return this.isEmpty()?e.set(0,0,0):e.addVectors(this.min,this.max).multiplyScalar(.5)}getSize(e){return this.isEmpty()?e.set(0,0,0):e.subVectors(this.max,this.min)}expandByPoint(e){return this.min.min(e),this.max.max(e),this}expandByVector(e){return this.min.sub(e),this.max.add(e),this}expandByScalar(e){return this.min.addScalar(-e),this.max.addScalar(e),this}expandByObject(e,t=!1){e.updateWorldMatrix(!1,!1);let i=e.geometry;if(i!==void 0){let s=i.getAttribute("position");if(t===!0&&s!==void 0&&e.isInstancedMesh!==!0)for(let o=0,a=s.count;o<a;o++)e.isMesh===!0?e.getVertexPosition(o,Qn):Qn.fromBufferAttribute(s,o),Qn.applyMatrix4(e.matrixWorld),this.expandByPoint(Qn);else e.boundingBox!==void 0?(e.boundingBox===null&&e.computeBoundingBox(),ko.copy(e.boundingBox)):(i.boundingBox===null&&i.computeBoundingBox(),ko.copy(i.boundingBox)),ko.applyMatrix4(e.matrixWorld),this.union(ko)}let r=e.children;for(let s=0,o=r.length;s<o;s++)this.expandByObject(r[s],t);return this}containsPoint(e){return e.x>=this.min.x&&e.x<=this.max.x&&e.y>=this.min.y&&e.y<=this.max.y&&e.z>=this.min.z&&e.z<=this.max.z}containsBox(e){return this.min.x<=e.min.x&&e.max.x<=this.max.x&&this.min.y<=e.min.y&&e.max.y<=this.max.y&&this.min.z<=e.min.z&&e.max.z<=this.max.z}getParameter(e,t){return t.set((e.x-this.min.x)/(this.max.x-this.min.x),(e.y-this.min.y)/(this.max.y-this.min.y),(e.z-this.min.z)/(this.max.z-this.min.z))}intersectsBox(e){return e.max.x>=this.min.x&&e.min.x<=this.max.x&&e.max.y>=this.min.y&&e.min.y<=this.max.y&&e.max.z>=this.min.z&&e.min.z<=this.max.z}intersectsSphere(e){return this.clampPoint(e.center,Qn),Qn.distanceToSquared(e.center)<=e.radius*e.radius}intersectsPlane(e){let t,i;return e.normal.x>0?(t=e.normal.x*this.min.x,i=e.normal.x*this.max.x):(t=e.normal.x*this.max.x,i=e.normal.x*this.min.x),e.normal.y>0?(t+=e.normal.y*this.min.y,i+=e.normal.y*this.max.y):(t+=e.normal.y*this.max.y,i+=e.normal.y*this.min.y),e.normal.z>0?(t+=e.normal.z*this.min.z,i+=e.normal.z*this.max.z):(t+=e.normal.z*this.max.z,i+=e.normal.z*this.min.z),t<=-e.constant&&i>=-e.constant}intersectsTriangle(e){if(this.isEmpty())return!1;this.getCenter(Ms),Do.subVectors(this.max,Ms),Nr.subVectors(e.a,Ms),Ur.subVectors(e.b,Ms),Fr.subVectors(e.c,Ms),Ui.subVectors(Ur,Nr),Fi.subVectors(Fr,Ur),ar.subVectors(Nr,Fr);let t=[0,-Ui.z,Ui.y,0,-Fi.z,Fi.y,0,-ar.z,ar.y,Ui.z,0,-Ui.x,Fi.z,0,-Fi.x,ar.z,0,-ar.x,-Ui.y,Ui.x,0,-Fi.y,Fi.x,0,-ar.y,ar.x,0];return!cc(t,Nr,Ur,Fr,Do)||(t=[1,0,0,0,1,0,0,0,1],!cc(t,Nr,Ur,Fr,Do))?!1:(No.crossVectors(Ui,Fi),t=[No.x,No.y,No.z],cc(t,Nr,Ur,Fr,Do))}clampPoint(e,t){return t.copy(e).clamp(this.min,this.max)}distanceToPoint(e){return this.clampPoint(e,Qn).distanceTo(e)}getBoundingSphere(e){return this.isEmpty()?e.makeEmpty():(this.getCenter(e.center),e.radius=this.getSize(Qn).length()*.5),e}intersect(e){return this.min.max(e.min),this.max.min(e.max),this.isEmpty()&&this.makeEmpty(),this}union(e){return this.min.min(e.min),this.max.max(e.max),this}applyMatrix4(e){return this.isEmpty()?this:(Mi[0].set(this.min.x,this.min.y,this.min.z).applyMatrix4(e),Mi[1].set(this.min.x,this.min.y,this.max.z).applyMatrix4(e),Mi[2].set(this.min.x,this.max.y,this.min.z).applyMatrix4(e),Mi[3].set(this.min.x,this.max.y,this.max.z).applyMatrix4(e),Mi[4].set(this.max.x,this.min.y,this.min.z).applyMatrix4(e),Mi[5].set(this.max.x,this.min.y,this.max.z).applyMatrix4(e),Mi[6].set(this.max.x,this.max.y,this.min.z).applyMatrix4(e),Mi[7].set(this.max.x,this.max.y,this.max.z).applyMatrix4(e),this.setFromPoints(Mi),this)}translate(e){return this.min.add(e),this.max.add(e),this}equals(e){return e.min.equals(this.min)&&e.max.equals(this.max)}toJSON(){return{min:this.min.toArray(),max:this.max.toArray()}}fromJSON(e){return this.min.fromArray(e.min),this.max.fromArray(e.max),this}},Mi=[new G,new G,new G,new G,new G,new G,new G,new G],Qn=new G,ko=new Dn,Nr=new G,Ur=new G,Fr=new G,Ui=new G,Fi=new G,ar=new G,Ms=new G,Do=new G,No=new G,lr=new G;function cc(n,e,t,i,r){for(let s=0,o=n.length-3;s<=o;s+=3){lr.fromArray(n,s);let a=r.x*Math.abs(lr.x)+r.y*Math.abs(lr.y)+r.z*Math.abs(lr.z),l=e.dot(lr),c=t.dot(lr),d=i.dot(lr);if(Math.max(-Math.max(l,c,d),Math.min(l,c,d))>a)return!1}return!0}var sn=new G,Uo=new We,Wp=0,Pn=class extends ti{constructor(e,t,i=!1){if(super(),Array.isArray(e))throw new TypeError("THREE.BufferAttribute: array should be a Typed Array.");this.isBufferAttribute=!0,Object.defineProperty(this,"id",{value:Wp++}),this.name="",this.array=e,this.itemSize=t,this.count=e!==void 0?e.length/t:0,this.normalized=i,this.usage=cd,this.updateRanges=[],this.gpuType=$n,this.version=0}onUploadCallback(){}set needsUpdate(e){e===!0&&this.version++}setUsage(e){return this.usage=e,this}addUpdateRange(e,t){this.updateRanges.push({start:e,count:t})}clearUpdateRanges(){this.updateRanges.length=0}copy(e){return this.name=e.name,this.array=new e.array.constructor(e.array),this.itemSize=e.itemSize,this.count=e.count,this.normalized=e.normalized,this.usage=e.usage,this.gpuType=e.gpuType,this}copyAt(e,t,i){e*=this.itemSize,i*=t.itemSize;for(let r=0,s=this.itemSize;r<s;r++)this.array[e+r]=t.array[i+r];return this}copyArray(e){return this.array.set(e),this}applyMatrix3(e){if(this.itemSize===2)for(let t=0,i=this.count;t<i;t++)Uo.fromBufferAttribute(this,t),Uo.applyMatrix3(e),this.setXY(t,Uo.x,Uo.y);else if(this.itemSize===3)for(let t=0,i=this.count;t<i;t++)sn.fromBufferAttribute(this,t),sn.applyMatrix3(e),this.setXYZ(t,sn.x,sn.y,sn.z);return this}applyMatrix4(e){for(let t=0,i=this.count;t<i;t++)sn.fromBufferAttribute(this,t),sn.applyMatrix4(e),this.setXYZ(t,sn.x,sn.y,sn.z);return this}applyNormalMatrix(e){for(let t=0,i=this.count;t<i;t++)sn.fromBufferAttribute(this,t),sn.applyNormalMatrix(e),this.setXYZ(t,sn.x,sn.y,sn.z);return this}transformDirection(e){for(let t=0,i=this.count;t<i;t++)sn.fromBufferAttribute(this,t),sn.transformDirection(e),this.setXYZ(t,sn.x,sn.y,sn.z);return this}set(e,t=0){return this.array.set(e,t),this}getComponent(e,t){let i=this.array[e*this.itemSize+t];return this.normalized&&(i=Vr(i,this.array)),i}setComponent(e,t,i){return this.normalized&&(i=Cn(i,this.array)),this.array[e*this.itemSize+t]=i,this}getX(e){let t=this.array[e*this.itemSize];return this.normalized&&(t=Vr(t,this.array)),t}setX(e,t){return this.normalized&&(t=Cn(t,this.array)),this.array[e*this.itemSize]=t,this}getY(e){let t=this.array[e*this.itemSize+1];return this.normalized&&(t=Vr(t,this.array)),t}setY(e,t){return this.normalized&&(t=Cn(t,this.array)),this.array[e*this.itemSize+1]=t,this}getZ(e){let t=this.array[e*this.itemSize+2];return this.normalized&&(t=Vr(t,this.array)),t}setZ(e,t){return this.normalized&&(t=Cn(t,this.array)),this.array[e*this.itemSize+2]=t,this}getW(e){let t=this.array[e*this.itemSize+3];return this.normalized&&(t=Vr(t,this.array)),t}setW(e,t){return this.normalized&&(t=Cn(t,this.array)),this.array[e*this.itemSize+3]=t,this}setXY(e,t,i){return e*=this.itemSize,this.normalized&&(t=Cn(t,this.array),i=Cn(i,this.array)),this.array[e+0]=t,this.array[e+1]=i,this}setXYZ(e,t,i,r){return e*=this.itemSize,this.normalized&&(t=Cn(t,this.array),i=Cn(i,this.array),r=Cn(r,this.array)),this.array[e+0]=t,this.array[e+1]=i,this.array[e+2]=r,this}setXYZW(e,t,i,r,s){return e*=this.itemSize,this.normalized&&(t=Cn(t,this.array),i=Cn(i,this.array),r=Cn(r,this.array),s=Cn(s,this.array)),this.array[e+0]=t,this.array[e+1]=i,this.array[e+2]=r,this.array[e+3]=s,this}onUpload(e){return this.onUploadCallback=e,this}clone(){return new this.constructor(this.array,this.itemSize).copy(this)}toJSON(){let e={itemSize:this.itemSize,type:this.array.constructor.name,array:Array.from(this.array),normalized:this.normalized};return e.name=this.name,e.usage=this.usage,e.gpuType=this.gpuType,e}dispose(){this.dispatchEvent({type:"dispose"})}};var Us=class extends Pn{constructor(e,t,i){super(new Uint16Array(e),t,i)}};var Fs=class extends Pn{constructor(e,t,i){super(new Uint32Array(e),t,i)}};var bt=class extends Pn{constructor(e,t,i){super(new Float32Array(e),t,i)}},Xp=new Dn,Ss=new G,uc=new G,Ei=class{constructor(e=new G,t=-1){this.isSphere=!0,this.center=e,this.radius=t}set(e,t){return this.center.copy(e),this.radius=t,this}setFromPoints(e,t){let i=this.center;t!==void 0?i.copy(t):Xp.setFromPoints(e).getCenter(i);let r=0;for(let s=0,o=e.length;s<o;s++)r=Math.max(r,i.distanceToSquared(e[s]));return this.radius=Math.sqrt(r),this}copy(e){return this.center.copy(e.center),this.radius=e.radius,this}isEmpty(){return this.radius<0}makeEmpty(){return this.center.set(0,0,0),this.radius=-1,this}containsPoint(e){return e.distanceToSquared(this.center)<=this.radius*this.radius}distanceToPoint(e){return e.distanceTo(this.center)-this.radius}intersectsSphere(e){let t=this.radius+e.radius;return e.center.distanceToSquared(this.center)<=t*t}intersectsBox(e){return e.intersectsSphere(this)}intersectsPlane(e){return Math.abs(e.distanceToPoint(this.center))<=this.radius}clampPoint(e,t){let i=this.center.distanceToSquared(e);return t.copy(e),i>this.radius*this.radius&&(t.sub(this.center).normalize(),t.multiplyScalar(this.radius).add(this.center)),t}getBoundingBox(e){return this.isEmpty()?(e.makeEmpty(),e):(e.set(this.center,this.center),e.expandByScalar(this.radius),e)}applyMatrix4(e){return this.center.applyMatrix4(e),this.radius=this.radius*e.getMaxScaleOnAxis(),this}translate(e){return this.center.add(e),this}expandByPoint(e){if(this.isEmpty())return this.center.copy(e),this.radius=0,this;Ss.subVectors(e,this.center);let t=Ss.lengthSq();if(t>this.radius*this.radius){let i=Math.sqrt(t),r=(i-this.radius)*.5;this.center.addScaledVector(Ss,r/i),this.radius+=r}return this}union(e){return e.isEmpty()?this:this.isEmpty()?(this.copy(e),this):(this.center.equals(e.center)===!0?this.radius=Math.max(this.radius,e.radius):(uc.subVectors(e.center,this.center).setLength(e.radius),this.expandByPoint(Ss.copy(e.center).add(uc)),this.expandByPoint(Ss.copy(e.center).sub(uc))),this)}equals(e){return e.center.equals(this.center)&&e.radius===this.radius}clone(){return new this.constructor().copy(this)}toJSON(){return{radius:this.radius,center:this.center.toArray()}}fromJSON(e){return this.radius=e.radius,this.center.fromArray(e.center),this}},qp=0,qn=new wt,hc=new vn,Or=new G,zn=new Dn,ws=new Dn,dn=new G,$t=class n extends ti{constructor(){super(),this.isBufferGeometry=!0,Object.defineProperty(this,"id",{value:qp++}),this.uuid=cs(),this.name="",this.type="BufferGeometry",this.index=null,this.indirect=null,this.indirectOffset=0,this.attributes={},this.morphAttributes={},this.morphTargetsRelative=!1,this.groups=[],this.boundingBox=null,this.boundingSphere=null,this.drawRange={start:0,count:1/0},this.userData={},this._transformed=!1}getIndex(){return this.index}setIndex(e){return Array.isArray(e)?this.index=new(gp(e)?Fs:Us)(e,1):this.index=e,this}setIndirect(e,t=0){return this.indirect=e,this.indirectOffset=t,this}getIndirect(){return this.indirect}getAttribute(e){return this.attributes[e]}setAttribute(e,t){return this.attributes[e]=t,this}deleteAttribute(e){return delete this.attributes[e],this}hasAttribute(e){return this.attributes[e]!==void 0}addGroup(e,t,i=0){this.groups.push({start:e,count:t,materialIndex:i})}clearGroups(){this.groups=[]}setDrawRange(e,t){this.drawRange.start=e,this.drawRange.count=t}applyMatrix4(e){let t=this.attributes.position;t!==void 0&&(t.applyMatrix4(e),t.needsUpdate=!0);let i=this.attributes.normal;if(i!==void 0){let s=new je().getNormalMatrix(e);i.applyNormalMatrix(s),i.needsUpdate=!0}let r=this.attributes.tangent;return r!==void 0&&(r.transformDirection(e),r.needsUpdate=!0),this.boundingBox!==null&&this.computeBoundingBox(),this.boundingSphere!==null&&this.computeBoundingSphere(),this._transformed=!0,this}applyQuaternion(e){return qn.makeRotationFromQuaternion(e),this.applyMatrix4(qn),this}rotateX(e){return qn.makeRotationX(e),this.applyMatrix4(qn),this}rotateY(e){return qn.makeRotationY(e),this.applyMatrix4(qn),this}rotateZ(e){return qn.makeRotationZ(e),this.applyMatrix4(qn),this}translate(e,t,i){return qn.makeTranslation(e,t,i),this.applyMatrix4(qn),this}scale(e,t,i){return qn.makeScale(e,t,i),this.applyMatrix4(qn),this}lookAt(e){return hc.lookAt(e),hc.updateMatrix(),this.applyMatrix4(hc.matrix),this}center(){return this.computeBoundingBox(),this.boundingBox.getCenter(Or).negate(),this.translate(Or.x,Or.y,Or.z),this}setFromPoints(e){let t=this.getAttribute("position");if(t===void 0){let i=[];for(let r=0,s=e.length;r<s;r++){let o=e[r];i.push(o.x,o.y,o.z||0)}this.setAttribute("position",new bt(i,3))}else{let i=Math.min(e.length,t.count);for(let r=0;r<i;r++){let s=e[r];t.setXYZ(r,s.x,s.y,s.z||0)}e.length>t.count&&$e("BufferGeometry: Buffer size too small for points data. Use .dispose() and create a new geometry."),t.needsUpdate=!0}return this}computeBoundingBox(){this.boundingBox===null&&(this.boundingBox=new Dn);let e=this.attributes.position,t=this.morphAttributes.position;if(e&&e.isGLBufferAttribute){Ze("BufferGeometry.computeBoundingBox(): GLBufferAttribute requires a manual bounding box.",this),this.boundingBox.set(new G(-1/0,-1/0,-1/0),new G(1/0,1/0,1/0));return}if(e!==void 0){if(this.boundingBox.setFromBufferAttribute(e),t)for(let i=0,r=t.length;i<r;i++){let s=t[i];zn.setFromBufferAttribute(s),this.morphTargetsRelative?(dn.addVectors(this.boundingBox.min,zn.min),this.boundingBox.expandByPoint(dn),dn.addVectors(this.boundingBox.max,zn.max),this.boundingBox.expandByPoint(dn)):(this.boundingBox.expandByPoint(zn.min),this.boundingBox.expandByPoint(zn.max))}}else this.boundingBox.makeEmpty();(isNaN(this.boundingBox.min.x)||isNaN(this.boundingBox.min.y)||isNaN(this.boundingBox.min.z))&&Ze('BufferGeometry.computeBoundingBox(): Computed min/max have NaN values. The "position" attribute is likely to have NaN values.',this)}computeBoundingSphere(){this.boundingSphere===null&&(this.boundingSphere=new Ei);let e=this.attributes.position,t=this.morphAttributes.position;if(e&&e.isGLBufferAttribute){Ze("BufferGeometry.computeBoundingSphere(): GLBufferAttribute requires a manual bounding sphere.",this),this.boundingSphere.set(new G,1/0);return}if(e){let i=this.boundingSphere.center;if(zn.setFromBufferAttribute(e),t)for(let s=0,o=t.length;s<o;s++){let a=t[s];ws.setFromBufferAttribute(a),this.morphTargetsRelative?(dn.addVectors(zn.min,ws.min),zn.expandByPoint(dn),dn.addVectors(zn.max,ws.max),zn.expandByPoint(dn)):(zn.expandByPoint(ws.min),zn.expandByPoint(ws.max))}zn.getCenter(i);let r=0;for(let s=0,o=e.count;s<o;s++)dn.fromBufferAttribute(e,s),r=Math.max(r,i.distanceToSquared(dn));if(t)for(let s=0,o=t.length;s<o;s++){let a=t[s],l=this.morphTargetsRelative;for(let c=0,d=a.count;c<d;c++)dn.fromBufferAttribute(a,c),l&&(Or.fromBufferAttribute(e,c),dn.add(Or)),r=Math.max(r,i.distanceToSquared(dn))}this.boundingSphere.radius=Math.sqrt(r),isNaN(this.boundingSphere.radius)&&Ze('BufferGeometry.computeBoundingSphere(): Computed radius is NaN. The "position" attribute is likely to have NaN values.',this)}}computeTangents(){let e=this.index,t=this.attributes;if(e===null||t.position===void 0||t.normal===void 0||t.uv===void 0){Ze("BufferGeometry: .computeTangents() failed. Missing required attributes (index, position, normal or uv)");return}let i=t.position,r=t.normal,s=t.uv,o=this.getAttribute("tangent");(o===void 0||o.count!==i.count)&&(o=new Pn(new Float32Array(4*i.count),4),this.setAttribute("tangent",o));let a=[],l=[];for(let p=0;p<i.count;p++)a[p]=new G,l[p]=new G;let c=new G,d=new G,m=new G,u=new We,g=new We,_=new We,v=new G,f=new G;function h(p,S,y){c.fromBufferAttribute(i,p),d.fromBufferAttribute(i,S),m.fromBufferAttribute(i,y),u.fromBufferAttribute(s,p),g.fromBufferAttribute(s,S),_.fromBufferAttribute(s,y),d.sub(c),m.sub(c),g.sub(u),_.sub(u);let R=1/(g.x*_.y-_.x*g.y);isFinite(R)&&(v.copy(d).multiplyScalar(_.y).addScaledVector(m,-g.y).multiplyScalar(R),f.copy(m).multiplyScalar(g.x).addScaledVector(d,-_.x).multiplyScalar(R),a[p].add(v),a[S].add(v),a[y].add(v),l[p].add(f),l[S].add(f),l[y].add(f))}let M=this.groups;M.length===0&&(M=[{start:0,count:e.count}]);for(let p=0,S=M.length;p<S;++p){let y=M[p],R=y.start,I=y.count;for(let k=R,D=R+I;k<D;k+=3)h(e.getX(k+0),e.getX(k+1),e.getX(k+2))}let P=new G,w=new G,T=new G,E=new G;function U(p){T.fromBufferAttribute(r,p),E.copy(T);let S=a[p];P.copy(S),P.sub(T.multiplyScalar(T.dot(S))).normalize(),w.crossVectors(E,S);let R=w.dot(l[p])<0?-1:1;o.setXYZW(p,P.x,P.y,P.z,R)}for(let p=0,S=M.length;p<S;++p){let y=M[p],R=y.start,I=y.count;for(let k=R,D=R+I;k<D;k+=3)U(e.getX(k+0)),U(e.getX(k+1)),U(e.getX(k+2))}this._transformed=!0}computeVertexNormals(){let e=this.index,t=this.getAttribute("position");if(t!==void 0){let i=this.getAttribute("normal");if(i===void 0||i.count!==t.count)i=new Pn(new Float32Array(t.count*3),3),this.setAttribute("normal",i);else for(let u=0,g=i.count;u<g;u++)i.setXYZ(u,0,0,0);let r=new G,s=new G,o=new G,a=new G,l=new G,c=new G,d=new G,m=new G;if(e)for(let u=0,g=e.count;u<g;u+=3){let _=e.getX(u+0),v=e.getX(u+1),f=e.getX(u+2);r.fromBufferAttribute(t,_),s.fromBufferAttribute(t,v),o.fromBufferAttribute(t,f),d.subVectors(o,s),m.subVectors(r,s),d.cross(m),a.fromBufferAttribute(i,_),l.fromBufferAttribute(i,v),c.fromBufferAttribute(i,f),a.add(d),l.add(d),c.add(d),i.setXYZ(_,a.x,a.y,a.z),i.setXYZ(v,l.x,l.y,l.z),i.setXYZ(f,c.x,c.y,c.z)}else for(let u=0,g=t.count;u<g;u+=3)r.fromBufferAttribute(t,u+0),s.fromBufferAttribute(t,u+1),o.fromBufferAttribute(t,u+2),d.subVectors(o,s),m.subVectors(r,s),d.cross(m),i.setXYZ(u+0,d.x,d.y,d.z),i.setXYZ(u+1,d.x,d.y,d.z),i.setXYZ(u+2,d.x,d.y,d.z);this.normalizeNormals(),i.needsUpdate=!0}}normalizeNormals(){let e=this.attributes.normal;for(let t=0,i=e.count;t<i;t++)dn.fromBufferAttribute(e,t),dn.normalize(),e.setXYZ(t,dn.x,dn.y,dn.z)}toNonIndexed(){function e(a,l){let c=a.array,d=a.itemSize,m=a.normalized,u=new c.constructor(l.length*d),g=0,_=0;for(let v=0,f=l.length;v<f;v++){a.isInterleavedBufferAttribute?g=l[v]*a.data.stride+a.offset:g=l[v]*d;for(let h=0;h<d;h++)u[_++]=c[g++]}return new Pn(u,d,m)}if(this.index===null)return $e("BufferGeometry.toNonIndexed(): BufferGeometry is already non-indexed."),this;let t=new n,i=this.index.array,r=this.attributes;for(let a in r){let l=r[a],c=e(l,i);t.setAttribute(a,c)}let s=this.morphAttributes;for(let a in s){let l=[],c=s[a];for(let d=0,m=c.length;d<m;d++){let u=c[d],g=e(u,i);l.push(g)}t.morphAttributes[a]=l}t.morphTargetsRelative=this.morphTargetsRelative;let o=this.groups;for(let a=0,l=o.length;a<l;a++){let c=o[a];t.addGroup(c.start,c.count,c.materialIndex)}return t}toJSON(){let e={metadata:{version:4.7,type:"BufferGeometry",generator:"BufferGeometry.toJSON"}};if(e.uuid=this.uuid,e.type=this.parameters!==void 0&&this._transformed===!0?"BufferGeometry":this.type,e.name=this.name,Object.keys(this.userData).length>0&&(e.userData=this.userData),this.parameters!==void 0&&this._transformed!==!0){let l=this.parameters;for(let c in l)l[c]!==void 0&&(e[c]=l[c]);return e}e.data={attributes:{}};let t=this.index;t!==null&&(e.data.index={type:t.array.constructor.name,array:Array.prototype.slice.call(t.array)});let i=this.attributes;for(let l in i){let c=i[l];e.data.attributes[l]=c.toJSON(e.data)}let r={},s=!1;for(let l in this.morphAttributes){let c=this.morphAttributes[l],d=[];for(let m=0,u=c.length;m<u;m++){let g=c[m];d.push(g.toJSON(e.data))}d.length>0&&(r[l]=d,s=!0)}s&&(e.data.morphAttributes=r,e.data.morphTargetsRelative=this.morphTargetsRelative);let o=this.groups;o.length>0&&(e.data.groups=JSON.parse(JSON.stringify(o)));let a=this.boundingSphere;return a!==null&&(e.data.boundingSphere=a.toJSON()),e}clone(){return new this.constructor().copy(this)}copy(e){this.index=null,this.attributes={},this.morphAttributes={},this.groups=[],this.boundingBox=null,this.boundingSphere=null;let t={};this.name=e.name;let i=e.index;i!==null&&this.setIndex(i.clone());let r=e.attributes;for(let c in r){let d=r[c];this.setAttribute(c,d.clone(t))}let s=e.morphAttributes;for(let c in s){let d=[],m=s[c];for(let u=0,g=m.length;u<g;u++)d.push(m[u].clone(t));this.morphAttributes[c]=d}this.morphTargetsRelative=e.morphTargetsRelative;let o=e.groups;for(let c=0,d=o.length;c<d;c++){let m=o[c];this.addGroup(m.start,m.count,m.materialIndex)}let a=e.boundingBox;a!==null&&(this.boundingBox=a.clone());let l=e.boundingSphere;return l!==null&&(this.boundingSphere=l.clone()),this.drawRange.start=e.drawRange.start,this.drawRange.count=e.drawRange.count,this.userData=e.userData,this._transformed=e._transformed,this}dispose(){this.dispatchEvent({type:"dispose"})}};var dc=new G,$p=new G,Yp=new je,Hn=class{constructor(e=new G(1,0,0),t=0){this.isPlane=!0,this.normal=e,this.constant=t}set(e,t){return this.normal.copy(e),this.constant=t,this}setComponents(e,t,i,r){return this.normal.set(e,t,i),this.constant=r,this}setFromNormalAndCoplanarPoint(e,t){return this.normal.copy(e),this.constant=-t.dot(this.normal),this}setFromCoplanarPoints(e,t,i){let r=dc.subVectors(i,t).cross($p.subVectors(e,t)).normalize();return this.setFromNormalAndCoplanarPoint(r,e),this}copy(e){return this.normal.copy(e.normal),this.constant=e.constant,this}normalize(){let e=1/this.normal.length();return this.normal.multiplyScalar(e),this.constant*=e,this}negate(){return this.constant*=-1,this.normal.negate(),this}distanceToPoint(e){return this.normal.dot(e)+this.constant}distanceToSphere(e){return this.distanceToPoint(e.center)-e.radius}projectPoint(e,t){return t.copy(e).addScaledVector(this.normal,-this.distanceToPoint(e))}intersectLine(e,t,i=!0){let r=e.delta(dc),s=this.normal.dot(r);if(s===0)return this.distanceToPoint(e.start)===0?t.copy(e.start):null;let o=-(e.start.dot(this.normal)+this.constant)/s;return i===!0&&(o<0||o>1)?null:t.copy(e.start).addScaledVector(r,o)}intersectsLine(e){let t=this.distanceToPoint(e.start),i=this.distanceToPoint(e.end);return t<0&&i>0||i<0&&t>0}intersectsBox(e){return e.intersectsPlane(this)}intersectsSphere(e){return e.intersectsPlane(this)}coplanarPoint(e){return e.copy(this.normal).multiplyScalar(-this.constant)}applyMatrix4(e,t){let i=t||Yp.getNormalMatrix(e),r=this.coplanarPoint(dc).applyMatrix4(e),s=this.normal.applyMatrix3(i).normalize();return this.constant=-r.dot(s),this}translate(e){return this.constant-=e.dot(this.normal),this}equals(e){return e.normal.equals(this.normal)&&e.constant===this.constant}clone(){return new this.constructor().copy(this)}toJSON(){return{normal:this.normal.toArray(),constant:this.constant}}fromJSON(e){return this.normal.fromArray(e.normal),this.constant=e.constant,this}},Zp=0,Ti=class extends ti{constructor(){super(),this.isMaterial=!0,Object.defineProperty(this,"id",{value:Zp++}),this.uuid=cs(),this.name="",this.type="Material",this.blending=ss,this.side=Ki,this.vertexColors=!1,this.opacity=1,this.transparent=!1,this.alphaHash=!1,this.blendSrc=Pc,this.blendDst=Ic,this.blendEquation=gr,this.blendSrcAlpha=null,this.blendDstAlpha=null,this.blendEquationAlpha=null,this.blendColor=new Ne(0,0,0),this.blendAlpha=0,this.depthFunc=Xr,this.depthTest=!0,this.depthWrite=!0,this.stencilWriteMask=255,this.stencilFunc=nd,this.stencilRef=0,this.stencilFuncMask=255,this.stencilFail=Qo,this.stencilZFail=Qo,this.stencilZPass=Qo,this.stencilWrite=!1,this.clippingPlanes=null,this.clipIntersection=!1,this.clipShadows=!1,this.shadowSide=null,this.colorWrite=!0,this.precision=null,this.polygonOffset=!1,this.polygonOffsetFactor=0,this.polygonOffsetUnits=0,this.dithering=!1,this.alphaToCoverage=!1,this.premultipliedAlpha=!1,this.forceSinglePass=!1,this.allowOverride=!0,this.visible=!0,this.toneMapped=!0,this.userData={},this.version=0,this._alphaTest=0}get alphaTest(){return this._alphaTest}set alphaTest(e){this._alphaTest>0!=e>0&&this.version++,this._alphaTest=e}onBeforeRender(){}onBeforeCompile(){}customProgramCacheKey(){return this.onBeforeCompile.toString()}setValues(e){if(e!==void 0)for(let t in e){let i=e[t];if(i===void 0){$e(`Material: parameter '${t}' has value of undefined.`);continue}let r=this[t];if(r===void 0){$e(`Material: '${t}' is not a property of THREE.${this.type}.`);continue}r&&r.isColor?r.set(i):r&&r.isVector2&&i&&i.isVector2||r&&r.isEuler&&i&&i.isEuler||r&&r.isVector3&&i&&i.isVector3?r.copy(i):this[t]=i}}toJSON(e){let t=e===void 0||typeof e=="string";t&&(e={textures:{},images:{}});let i={metadata:{version:4.7,type:"Material",generator:"Material.toJSON"}};i.uuid=this.uuid,i.type=this.type,i.blending=this.blending,i.side=this.side,i.shadowSide=this.shadowSide,i.vertexColors=this.vertexColors,i.opacity=this.opacity,i.transparent=this.transparent,i.blendSrc=this.blendSrc,i.blendDst=this.blendDst,i.blendEquation=this.blendEquation,i.blendSrcAlpha=this.blendSrcAlpha,i.blendDstAlpha=this.blendDstAlpha,i.blendEquationAlpha=this.blendEquationAlpha,i.blendColor=this.blendColor.getHex(),i.blendAlpha=this.blendAlpha,i.depthFunc=this.depthFunc,i.depthTest=this.depthTest,i.depthWrite=this.depthWrite,i.colorWrite=this.colorWrite,i.clipIntersection=this.clipIntersection,i.clipShadows=this.clipShadows,i.stencilWriteMask=this.stencilWriteMask,i.stencilFunc=this.stencilFunc,i.stencilRef=this.stencilRef,i.stencilFuncMask=this.stencilFuncMask,i.stencilFail=this.stencilFail,i.stencilZFail=this.stencilZFail,i.stencilZPass=this.stencilZPass,i.stencilWrite=this.stencilWrite,i.polygonOffset=this.polygonOffset,i.polygonOffsetFactor=this.polygonOffsetFactor,i.polygonOffsetUnits=this.polygonOffsetUnits,i.dithering=this.dithering,i.alphaTest=this.alphaTest,i.alphaHash=this.alphaHash,i.alphaToCoverage=this.alphaToCoverage,i.premultipliedAlpha=this.premultipliedAlpha,i.forceSinglePass=this.forceSinglePass,i.allowOverride=this.allowOverride,i.visible=this.visible,i.toneMapped=this.toneMapped,i.name=this.name,this.color&&this.color.isColor&&(i.color=this.color.getHex()),this.roughness!==void 0&&(i.roughness=this.roughness),this.metalness!==void 0&&(i.metalness=this.metalness),this.sheen!==void 0&&(i.sheen=this.sheen),this.sheenColor&&this.sheenColor.isColor&&(i.sheenColor=this.sheenColor.getHex()),this.sheenRoughness!==void 0&&(i.sheenRoughness=this.sheenRoughness),this.emissive&&this.emissive.isColor&&(i.emissive=this.emissive.getHex()),this.emissiveIntensity!==void 0&&(i.emissiveIntensity=this.emissiveIntensity),this.specular&&this.specular.isColor&&(i.specular=this.specular.getHex()),this.specularIntensity!==void 0&&(i.specularIntensity=this.specularIntensity),this.specularColor&&this.specularColor.isColor&&(i.specularColor=this.specularColor.getHex()),this.shininess!==void 0&&(i.shininess=this.shininess),this.clearcoat!==void 0&&(i.clearcoat=this.clearcoat),this.clearcoatRoughness!==void 0&&(i.clearcoatRoughness=this.clearcoatRoughness),this.clearcoatMap&&this.clearcoatMap.isTexture&&(i.clearcoatMap=this.clearcoatMap.toJSON(e).uuid),this.clearcoatRoughnessMap&&this.clearcoatRoughnessMap.isTexture&&(i.clearcoatRoughnessMap=this.clearcoatRoughnessMap.toJSON(e).uuid),this.clearcoatNormalMap&&this.clearcoatNormalMap.isTexture&&(i.clearcoatNormalMap=this.clearcoatNormalMap.toJSON(e).uuid,i.clearcoatNormalScale=this.clearcoatNormalScale.toArray()),this.sheenColorMap&&this.sheenColorMap.isTexture&&(i.sheenColorMap=this.sheenColorMap.toJSON(e).uuid),this.sheenRoughnessMap&&this.sheenRoughnessMap.isTexture&&(i.sheenRoughnessMap=this.sheenRoughnessMap.toJSON(e).uuid),this.dispersion!==void 0&&(i.dispersion=this.dispersion),this.retroreflectivity!==void 0&&(i.retroreflectivity=this.retroreflectivity),this.iridescence!==void 0&&(i.iridescence=this.iridescence),this.iridescenceIOR!==void 0&&(i.iridescenceIOR=this.iridescenceIOR),this.iridescenceThicknessRange!==void 0&&(i.iridescenceThicknessRange=this.iridescenceThicknessRange),this.iridescenceMap&&this.iridescenceMap.isTexture&&(i.iridescenceMap=this.iridescenceMap.toJSON(e).uuid),this.iridescenceThicknessMap&&this.iridescenceThicknessMap.isTexture&&(i.iridescenceThicknessMap=this.iridescenceThicknessMap.toJSON(e).uuid),this.anisotropy!==void 0&&(i.anisotropy=this.anisotropy),this.anisotropyRotation!==void 0&&(i.anisotropyRotation=this.anisotropyRotation),this.anisotropyMap&&this.anisotropyMap.isTexture&&(i.anisotropyMap=this.anisotropyMap.toJSON(e).uuid),this.map&&this.map.isTexture&&(i.map=this.map.toJSON(e).uuid),this.matcap&&this.matcap.isTexture&&(i.matcap=this.matcap.toJSON(e).uuid),this.alphaMap&&this.alphaMap.isTexture&&(i.alphaMap=this.alphaMap.toJSON(e).uuid),this.lightMap&&this.lightMap.isTexture&&(i.lightMap=this.lightMap.toJSON(e).uuid,i.lightMapIntensity=this.lightMapIntensity),this.aoMap&&this.aoMap.isTexture&&(i.aoMap=this.aoMap.toJSON(e).uuid,i.aoMapIntensity=this.aoMapIntensity),this.bumpMap&&this.bumpMap.isTexture&&(i.bumpMap=this.bumpMap.toJSON(e).uuid,i.bumpScale=this.bumpScale),this.normalMap&&this.normalMap.isTexture&&(i.normalMap=this.normalMap.toJSON(e).uuid,i.normalMapType=this.normalMapType,i.normalScale=this.normalScale.toArray()),this.displacementMap&&this.displacementMap.isTexture&&(i.displacementMap=this.displacementMap.toJSON(e).uuid,i.displacementScale=this.displacementScale,i.displacementBias=this.displacementBias),this.roughnessMap&&this.roughnessMap.isTexture&&(i.roughnessMap=this.roughnessMap.toJSON(e).uuid),this.metalnessMap&&this.metalnessMap.isTexture&&(i.metalnessMap=this.metalnessMap.toJSON(e).uuid),this.emissiveMap&&this.emissiveMap.isTexture&&(i.emissiveMap=this.emissiveMap.toJSON(e).uuid),this.specularMap&&this.specularMap.isTexture&&(i.specularMap=this.specularMap.toJSON(e).uuid),this.specularIntensityMap&&this.specularIntensityMap.isTexture&&(i.specularIntensityMap=this.specularIntensityMap.toJSON(e).uuid),this.specularColorMap&&this.specularColorMap.isTexture&&(i.specularColorMap=this.specularColorMap.toJSON(e).uuid),this.envMap&&this.envMap.isTexture&&(i.envMap=this.envMap.toJSON(e).uuid,this.combine!==void 0&&(i.combine=this.combine)),this.envMapRotation!==void 0&&(i.envMapRotation=this.envMapRotation.toArray()),this.envMapIntensity!==void 0&&(i.envMapIntensity=this.envMapIntensity),this.reflectivity!==void 0&&(i.reflectivity=this.reflectivity),this.refractionRatio!==void 0&&(i.refractionRatio=this.refractionRatio),this.gradientMap&&this.gradientMap.isTexture&&(i.gradientMap=this.gradientMap.toJSON(e).uuid),this.transmission!==void 0&&(i.transmission=this.transmission),this.transmissionMap&&this.transmissionMap.isTexture&&(i.transmissionMap=this.transmissionMap.toJSON(e).uuid),this.thickness!==void 0&&(i.thickness=this.thickness),this.thicknessMap&&this.thicknessMap.isTexture&&(i.thicknessMap=this.thicknessMap.toJSON(e).uuid),this.attenuationDistance!==void 0&&(i.attenuationDistance=this.attenuationDistance),this.attenuationColor!==void 0&&(i.attenuationColor=this.attenuationColor.getHex()),this.size!==void 0&&(i.size=this.size),this.sizeAttenuation!==void 0&&(i.sizeAttenuation=this.sizeAttenuation),Array.isArray(this.clippingPlanes)&&this.clippingPlanes.length>0&&(i.clippingPlanes=this.clippingPlanes.map(s=>s.toJSON())),this.rotation!==void 0&&(i.rotation=this.rotation),this.depthPacking!==void 0&&(i.depthPacking=this.depthPacking),this.linewidth!==void 0&&(i.linewidth=this.linewidth),this.linecap!==void 0&&(i.linecap=this.linecap),this.linejoin!==void 0&&(i.linejoin=this.linejoin),this.dashSize!==void 0&&(i.dashSize=this.dashSize),this.gapSize!==void 0&&(i.gapSize=this.gapSize),this.scale!==void 0&&(i.scale=this.scale),this.wireframe!==void 0&&(i.wireframe=this.wireframe),this.wireframeLinewidth!==void 0&&(i.wireframeLinewidth=this.wireframeLinewidth),this.wireframeLinecap!==void 0&&(i.wireframeLinecap=this.wireframeLinecap),this.wireframeLinejoin!==void 0&&(i.wireframeLinejoin=this.wireframeLinejoin),this.flatShading!==void 0&&(i.flatShading=this.flatShading),this.fog!==void 0&&(i.fog=this.fog),Object.keys(this.userData).length>0&&(i.userData=this.userData);function r(s){let o=[];for(let a in s){let l=s[a];delete l.metadata,o.push(l)}return o}if(t){let s=r(e.textures),o=r(e.images);s.length>0&&(i.textures=s),o.length>0&&(i.images=o)}return i}fromJSON(e,t){if(e.uuid!==void 0&&(this.uuid=e.uuid),e.name!==void 0&&(this.name=e.name),e.color!==void 0&&this.color!==void 0&&this.color.setHex(e.color),e.roughness!==void 0&&(this.roughness=e.roughness),e.metalness!==void 0&&(this.metalness=e.metalness),e.sheen!==void 0&&(this.sheen=e.sheen),e.sheenColor!==void 0&&(this.sheenColor=new Ne().setHex(e.sheenColor)),e.sheenRoughness!==void 0&&(this.sheenRoughness=e.sheenRoughness),e.emissive!==void 0&&this.emissive!==void 0&&this.emissive.setHex(e.emissive),e.specular!==void 0&&this.specular!==void 0&&this.specular.setHex(e.specular),e.specularIntensity!==void 0&&(this.specularIntensity=e.specularIntensity),e.specularColor!==void 0&&this.specularColor!==void 0&&this.specularColor.setHex(e.specularColor),e.shininess!==void 0&&(this.shininess=e.shininess),e.clearcoat!==void 0&&(this.clearcoat=e.clearcoat),e.clearcoatRoughness!==void 0&&(this.clearcoatRoughness=e.clearcoatRoughness),e.dispersion!==void 0&&(this.dispersion=e.dispersion),e.retroreflectivity!==void 0&&(this.retroreflectivity=e.retroreflectivity),e.iridescence!==void 0&&(this.iridescence=e.iridescence),e.iridescenceIOR!==void 0&&(this.iridescenceIOR=e.iridescenceIOR),e.iridescenceThicknessRange!==void 0&&(this.iridescenceThicknessRange=e.iridescenceThicknessRange),e.transmission!==void 0&&(this.transmission=e.transmission),e.thickness!==void 0&&(this.thickness=e.thickness),e.attenuationDistance!==void 0&&(this.attenuationDistance=e.attenuationDistance),e.attenuationColor!==void 0&&this.attenuationColor!==void 0&&this.attenuationColor.setHex(e.attenuationColor),e.anisotropy!==void 0&&(this.anisotropy=e.anisotropy),e.anisotropyRotation!==void 0&&(this.anisotropyRotation=e.anisotropyRotation),e.fog!==void 0&&(this.fog=e.fog),e.flatShading!==void 0&&(this.flatShading=e.flatShading),e.blending!==void 0&&(this.blending=e.blending),e.combine!==void 0&&(this.combine=e.combine),e.side!==void 0&&(this.side=e.side),e.shadowSide!==void 0&&(this.shadowSide=e.shadowSide),e.opacity!==void 0&&(this.opacity=e.opacity),e.transparent!==void 0&&(this.transparent=e.transparent),e.alphaTest!==void 0&&(this.alphaTest=e.alphaTest),e.alphaHash!==void 0&&(this.alphaHash=e.alphaHash),e.depthFunc!==void 0&&(this.depthFunc=e.depthFunc),e.depthTest!==void 0&&(this.depthTest=e.depthTest),e.depthWrite!==void 0&&(this.depthWrite=e.depthWrite),e.colorWrite!==void 0&&(this.colorWrite=e.colorWrite),e.clippingPlanes!==void 0&&(this.clippingPlanes=e.clippingPlanes.map(i=>new Hn().fromJSON(i))),e.clipIntersection!==void 0&&(this.clipIntersection=e.clipIntersection),e.clipShadows!==void 0&&(this.clipShadows=e.clipShadows),e.depthPacking!==void 0&&(this.depthPacking=e.depthPacking),e.blendSrc!==void 0&&(this.blendSrc=e.blendSrc),e.blendDst!==void 0&&(this.blendDst=e.blendDst),e.blendEquation!==void 0&&(this.blendEquation=e.blendEquation),e.blendSrcAlpha!==void 0&&(this.blendSrcAlpha=e.blendSrcAlpha),e.blendDstAlpha!==void 0&&(this.blendDstAlpha=e.blendDstAlpha),e.blendEquationAlpha!==void 0&&(this.blendEquationAlpha=e.blendEquationAlpha),e.blendColor!==void 0&&this.blendColor!==void 0&&this.blendColor.setHex(e.blendColor),e.blendAlpha!==void 0&&(this.blendAlpha=e.blendAlpha),e.stencilWriteMask!==void 0&&(this.stencilWriteMask=e.stencilWriteMask),e.stencilFunc!==void 0&&(this.stencilFunc=e.stencilFunc),e.stencilRef!==void 0&&(this.stencilRef=e.stencilRef),e.stencilFuncMask!==void 0&&(this.stencilFuncMask=e.stencilFuncMask),e.stencilFail!==void 0&&(this.stencilFail=e.stencilFail),e.stencilZFail!==void 0&&(this.stencilZFail=e.stencilZFail),e.stencilZPass!==void 0&&(this.stencilZPass=e.stencilZPass),e.stencilWrite!==void 0&&(this.stencilWrite=e.stencilWrite),e.wireframe!==void 0&&(this.wireframe=e.wireframe),e.wireframeLinewidth!==void 0&&(this.wireframeLinewidth=e.wireframeLinewidth),e.wireframeLinecap!==void 0&&(this.wireframeLinecap=e.wireframeLinecap),e.wireframeLinejoin!==void 0&&(this.wireframeLinejoin=e.wireframeLinejoin),e.rotation!==void 0&&(this.rotation=e.rotation),e.linewidth!==void 0&&(this.linewidth=e.linewidth),e.linecap!==void 0&&(this.linecap=e.linecap),e.linejoin!==void 0&&(this.linejoin=e.linejoin),e.dashSize!==void 0&&(this.dashSize=e.dashSize),e.gapSize!==void 0&&(this.gapSize=e.gapSize),e.scale!==void 0&&(this.scale=e.scale),e.polygonOffset!==void 0&&(this.polygonOffset=e.polygonOffset),e.polygonOffsetFactor!==void 0&&(this.polygonOffsetFactor=e.polygonOffsetFactor),e.polygonOffsetUnits!==void 0&&(this.polygonOffsetUnits=e.polygonOffsetUnits),e.dithering!==void 0&&(this.dithering=e.dithering),e.alphaToCoverage!==void 0&&(this.alphaToCoverage=e.alphaToCoverage),e.premultipliedAlpha!==void 0&&(this.premultipliedAlpha=e.premultipliedAlpha),e.forceSinglePass!==void 0&&(this.forceSinglePass=e.forceSinglePass),e.allowOverride!==void 0&&(this.allowOverride=e.allowOverride),e.visible!==void 0&&(this.visible=e.visible),e.toneMapped!==void 0&&(this.toneMapped=e.toneMapped),e.userData!==void 0&&(this.userData=e.userData),e.vertexColors!==void 0&&(typeof e.vertexColors=="number"?this.vertexColors=e.vertexColors>0:this.vertexColors=e.vertexColors),e.size!==void 0&&(this.size=e.size),e.sizeAttenuation!==void 0&&(this.sizeAttenuation=e.sizeAttenuation),e.map!==void 0&&(this.map=t[e.map]||null),e.matcap!==void 0&&(this.matcap=t[e.matcap]||null),e.alphaMap!==void 0&&(this.alphaMap=t[e.alphaMap]||null),e.bumpMap!==void 0&&(this.bumpMap=t[e.bumpMap]||null),e.bumpScale!==void 0&&(this.bumpScale=e.bumpScale),e.normalMap!==void 0&&(this.normalMap=t[e.normalMap]||null),e.normalMapType!==void 0&&(this.normalMapType=e.normalMapType),e.normalScale!==void 0){let i=e.normalScale;Array.isArray(i)===!1&&(i=[i,i]),this.normalScale=new We().fromArray(i)}return e.displacementMap!==void 0&&(this.displacementMap=t[e.displacementMap]||null),e.displacementScale!==void 0&&(this.displacementScale=e.displacementScale),e.displacementBias!==void 0&&(this.displacementBias=e.displacementBias),e.roughnessMap!==void 0&&(this.roughnessMap=t[e.roughnessMap]||null),e.metalnessMap!==void 0&&(this.metalnessMap=t[e.metalnessMap]||null),e.emissiveMap!==void 0&&(this.emissiveMap=t[e.emissiveMap]||null),e.emissiveIntensity!==void 0&&(this.emissiveIntensity=e.emissiveIntensity),e.specularMap!==void 0&&(this.specularMap=t[e.specularMap]||null),e.specularIntensityMap!==void 0&&(this.specularIntensityMap=t[e.specularIntensityMap]||null),e.specularColorMap!==void 0&&(this.specularColorMap=t[e.specularColorMap]||null),e.envMap!==void 0&&(this.envMap=t[e.envMap]||null),e.envMapRotation!==void 0&&this.envMapRotation.fromArray(e.envMapRotation),e.envMapIntensity!==void 0&&(this.envMapIntensity=e.envMapIntensity),e.reflectivity!==void 0&&(this.reflectivity=e.reflectivity),e.refractionRatio!==void 0&&(this.refractionRatio=e.refractionRatio),e.lightMap!==void 0&&(this.lightMap=t[e.lightMap]||null),e.lightMapIntensity!==void 0&&(this.lightMapIntensity=e.lightMapIntensity),e.aoMap!==void 0&&(this.aoMap=t[e.aoMap]||null),e.aoMapIntensity!==void 0&&(this.aoMapIntensity=e.aoMapIntensity),e.gradientMap!==void 0&&(this.gradientMap=t[e.gradientMap]||null),e.clearcoatMap!==void 0&&(this.clearcoatMap=t[e.clearcoatMap]||null),e.clearcoatRoughnessMap!==void 0&&(this.clearcoatRoughnessMap=t[e.clearcoatRoughnessMap]||null),e.clearcoatNormalMap!==void 0&&(this.clearcoatNormalMap=t[e.clearcoatNormalMap]||null),e.clearcoatNormalScale!==void 0&&(this.clearcoatNormalScale=new We().fromArray(e.clearcoatNormalScale)),e.iridescenceMap!==void 0&&(this.iridescenceMap=t[e.iridescenceMap]||null),e.iridescenceThicknessMap!==void 0&&(this.iridescenceThicknessMap=t[e.iridescenceThicknessMap]||null),e.transmissionMap!==void 0&&(this.transmissionMap=t[e.transmissionMap]||null),e.thicknessMap!==void 0&&(this.thicknessMap=t[e.thicknessMap]||null),e.anisotropyMap!==void 0&&(this.anisotropyMap=t[e.anisotropyMap]||null),e.sheenColorMap!==void 0&&(this.sheenColorMap=t[e.sheenColorMap]||null),e.sheenRoughnessMap!==void 0&&(this.sheenRoughnessMap=t[e.sheenRoughnessMap]||null),this}clone(){return new this.constructor().copy(this)}copy(e){this.name=e.name,this.blending=e.blending,this.side=e.side,this.vertexColors=e.vertexColors,this.opacity=e.opacity,this.transparent=e.transparent,this.blendSrc=e.blendSrc,this.blendDst=e.blendDst,this.blendEquation=e.blendEquation,this.blendSrcAlpha=e.blendSrcAlpha,this.blendDstAlpha=e.blendDstAlpha,this.blendEquationAlpha=e.blendEquationAlpha,this.blendColor.copy(e.blendColor),this.blendAlpha=e.blendAlpha,this.depthFunc=e.depthFunc,this.depthTest=e.depthTest,this.depthWrite=e.depthWrite,this.stencilWriteMask=e.stencilWriteMask,this.stencilFunc=e.stencilFunc,this.stencilRef=e.stencilRef,this.stencilFuncMask=e.stencilFuncMask,this.stencilFail=e.stencilFail,this.stencilZFail=e.stencilZFail,this.stencilZPass=e.stencilZPass,this.stencilWrite=e.stencilWrite;let t=e.clippingPlanes,i=null;if(t!==null){let r=t.length;i=new Array(r);for(let s=0;s!==r;++s)i[s]=t[s].clone()}return this.clippingPlanes=i,this.clipIntersection=e.clipIntersection,this.clipShadows=e.clipShadows,this.shadowSide=e.shadowSide,this.colorWrite=e.colorWrite,this.precision=e.precision,this.polygonOffset=e.polygonOffset,this.polygonOffsetFactor=e.polygonOffsetFactor,this.polygonOffsetUnits=e.polygonOffsetUnits,this.dithering=e.dithering,this.alphaTest=e.alphaTest,this.alphaHash=e.alphaHash,this.alphaToCoverage=e.alphaToCoverage,this.premultipliedAlpha=e.premultipliedAlpha,this.forceSinglePass=e.forceSinglePass,this.allowOverride=e.allowOverride,this.visible=e.visible,this.toneMapped=e.toneMapped,this.userData=JSON.parse(JSON.stringify(e.userData)),this}dispose(){this.dispatchEvent({type:"dispose"})}set needsUpdate(e){e===!0&&this.version++}};var Si=new G,fc=new G,Fo=new G,Oo=new G,Hi=class{constructor(e=new G,t=new G(0,0,-1)){this.origin=e,this.direction=t}set(e,t){return this.origin.copy(e),this.direction.copy(t),this}copy(e){return this.origin.copy(e.origin),this.direction.copy(e.direction),this}at(e,t){return t.copy(this.origin).addScaledVector(this.direction,e)}lookAt(e){return this.direction.copy(e).sub(this.origin).normalize(),this}recast(e){return this.origin.copy(this.at(e,Si)),this}closestPointToPoint(e,t){t.subVectors(e,this.origin);let i=t.dot(this.direction);return i<0?t.copy(this.origin):t.copy(this.origin).addScaledVector(this.direction,i)}distanceToPoint(e){return Math.sqrt(this.distanceSqToPoint(e))}distanceSqToPoint(e){let t=Si.subVectors(e,this.origin).dot(this.direction);return t<0?this.origin.distanceToSquared(e):(Si.copy(this.origin).addScaledVector(this.direction,t),Si.distanceToSquared(e))}distanceSqToSegment(e,t,i,r){fc.copy(e).add(t).multiplyScalar(.5),Fo.copy(t).sub(e).normalize(),Oo.copy(this.origin).sub(fc);let s=e.distanceTo(t)*.5,o=-this.direction.dot(Fo),a=Oo.dot(this.direction),l=-Oo.dot(Fo),c=Oo.lengthSq(),d=Math.abs(1-o*o),m,u,g,_;if(d>0)if(m=o*l-a,u=o*a-l,_=s*d,m>=0)if(u>=-_)if(u<=_){let v=1/d;m*=v,u*=v,g=m*(m+o*u+2*a)+u*(o*m+u+2*l)+c}else u=s,m=Math.max(0,-(o*u+a)),g=-m*m+u*(u+2*l)+c;else u=-s,m=Math.max(0,-(o*u+a)),g=-m*m+u*(u+2*l)+c;else u<=-_?(m=Math.max(0,-(-o*s+a)),u=m>0?-s:Math.min(Math.max(-s,-l),s),g=-m*m+u*(u+2*l)+c):u<=_?(m=0,u=Math.min(Math.max(-s,-l),s),g=u*(u+2*l)+c):(m=Math.max(0,-(o*s+a)),u=m>0?s:Math.min(Math.max(-s,-l),s),g=-m*m+u*(u+2*l)+c);else u=o>0?-s:s,m=Math.max(0,-(o*u+a)),g=-m*m+u*(u+2*l)+c;return i&&i.copy(this.origin).addScaledVector(this.direction,m),r&&r.copy(fc).addScaledVector(Fo,u),g}intersectSphere(e,t){if(e.radius<0)return null;Si.subVectors(e.center,this.origin);let i=Si.dot(this.direction),r=Si.dot(Si)-i*i,s=e.radius*e.radius;if(r>s)return null;let o=Math.sqrt(s-r),a=i-o,l=i+o;return l<0?null:a<0?this.at(l,t):this.at(a,t)}intersectsSphere(e){return e.radius<0?!1:this.distanceSqToPoint(e.center)<=e.radius*e.radius}distanceToPlane(e){let t=e.normal.dot(this.direction);if(t===0)return e.distanceToPoint(this.origin)===0?0:null;let i=-(this.origin.dot(e.normal)+e.constant)/t;return i>=0?i:null}intersectPlane(e,t){let i=this.distanceToPlane(e);return i===null?null:this.at(i,t)}intersectsPlane(e){let t=e.distanceToPoint(this.origin);return t===0||e.normal.dot(this.direction)*t<0}intersectBox(e,t){let i,r,s,o,a,l,c=1/this.direction.x,d=1/this.direction.y,m=1/this.direction.z,u=this.origin;return c>=0?(i=(e.min.x-u.x)*c,r=(e.max.x-u.x)*c):(i=(e.max.x-u.x)*c,r=(e.min.x-u.x)*c),d>=0?(s=(e.min.y-u.y)*d,o=(e.max.y-u.y)*d):(s=(e.max.y-u.y)*d,o=(e.min.y-u.y)*d),i>o||s>r||((s>i||isNaN(i))&&(i=s),(o<r||isNaN(r))&&(r=o),m>=0?(a=(e.min.z-u.z)*m,l=(e.max.z-u.z)*m):(a=(e.max.z-u.z)*m,l=(e.min.z-u.z)*m),i>l||a>r)||((a>i||i!==i)&&(i=a),(l<r||r!==r)&&(r=l),r<0)?null:this.at(i>=0?i:r,t)}intersectsBox(e){return this.intersectBox(e,Si)!==null}intersectTriangle(e,t,i,r,s){let o=this.origin,a=this.direction,l=a.x,c=a.y,d=a.z,m=e.x-o.x,u=e.y-o.y,g=e.z-o.z,_=t.x-o.x,v=t.y-o.y,f=t.z-o.z,h=i.x-o.x,M=i.y-o.y,P=i.z-o.z,w=Math.abs(l),T=Math.abs(c),E=Math.abs(d),U,p,S,y,R,I,k,D,N,H,$,ee;if(w>=T&&w>=E?(S=l,I=m,N=_,ee=h,l>=0?(U=c,p=d,y=u,R=g,k=v,D=f,H=M,$=P):(U=d,p=c,y=g,R=u,k=f,D=v,H=P,$=M)):T>=E?(S=c,I=u,N=v,ee=M,c>=0?(U=d,p=l,y=g,R=m,k=f,D=_,H=P,$=h):(U=l,p=d,y=m,R=g,k=_,D=f,H=h,$=P)):(S=d,I=g,N=f,ee=P,d>=0?(U=l,p=c,y=m,R=u,k=_,D=v,H=h,$=M):(U=c,p=l,y=u,R=m,k=v,D=_,H=M,$=h)),S===0)return null;let J=U/S,ie=p/S,ce=1/S,we=y-J*I,pe=R-ie*I,Pe=k-J*N,Ve=D-ie*N,Oe=H-J*ee,oe=$-ie*ee,le=Oe*Ve-oe*Pe,be=we*oe-pe*Oe,Ge=Pe*pe-Ve*we;if(r){if(le<0||be<0||Ge<0)return null}else if((le<0||be<0||Ge<0)&&(le>0||be>0||Ge>0))return null;let Me=le+be+Ge;if(Me===0)return null;let ze=ce*(le*I+be*N+Ge*ee);return(Me>0?ze<0:ze>0)?null:this.at(ze/Me,s)}applyMatrix4(e){return this.origin.applyMatrix4(e),this.direction.transformDirection(e),this}equals(e){return e.origin.equals(this.origin)&&e.direction.equals(this.direction)}clone(){return new this.constructor().copy(this)}},ln=class extends Ti{constructor(e){super(),this.isMeshBasicMaterial=!0,this.type="MeshBasicMaterial",this.color=new Ne(16777215),this.map=null,this.lightMap=null,this.lightMapIntensity=1,this.aoMap=null,this.aoMapIntensity=1,this.specularMap=null,this.alphaMap=null,this.envMap=null,this.envMapRotation=new zi,this.combine=Lc,this.reflectivity=1,this.refractionRatio=.98,this.wireframe=!1,this.wireframeLinewidth=1,this.wireframeLinecap="round",this.wireframeLinejoin="round",this.fog=!0,this.setValues(e)}copy(e){return super.copy(e),this.color.copy(e.color),this.map=e.map,this.lightMap=e.lightMap,this.lightMapIntensity=e.lightMapIntensity,this.aoMap=e.aoMap,this.aoMapIntensity=e.aoMapIntensity,this.specularMap=e.specularMap,this.alphaMap=e.alphaMap,this.envMap=e.envMap,this.envMapRotation.copy(e.envMapRotation),this.combine=e.combine,this.reflectivity=e.reflectivity,this.refractionRatio=e.refractionRatio,this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this.wireframeLinecap=e.wireframeLinecap,this.wireframeLinejoin=e.wireframeLinejoin,this.fog=e.fog,this}},mh=new wt,cr=new Hi,Bo=new Ei,gh=new G,zo=new G,Ho=new G,Go=new G,pc=new G,Vo=new G,xh=new G,Wo=new G,rt=class extends vn{constructor(e=new $t,t=new ln){super(),this.isMesh=!0,this.type="Mesh",this.geometry=e,this.material=t,this.morphTargetDictionary=void 0,this.morphTargetInfluences=void 0,this.count=1,this.updateMorphTargets()}copy(e,t){return super.copy(e,t),e.morphTargetInfluences!==void 0&&(this.morphTargetInfluences=e.morphTargetInfluences.slice()),e.morphTargetDictionary!==void 0&&(this.morphTargetDictionary=Object.assign({},e.morphTargetDictionary)),this.material=Array.isArray(e.material)?e.material.slice():e.material,this.geometry=e.geometry,this}updateMorphTargets(){let t=this.geometry.morphAttributes,i=Object.keys(t);if(i.length>0){let r=t[i[0]];if(r!==void 0){this.morphTargetInfluences=[],this.morphTargetDictionary={};for(let s=0,o=r.length;s<o;s++){let a=r[s].name||String(s);this.morphTargetInfluences.push(0),this.morphTargetDictionary[a]=s}}}}getVertexPosition(e,t){let i=this.geometry,r=i.attributes.position,s=i.morphAttributes.position,o=i.morphTargetsRelative;t.fromBufferAttribute(r,e);let a=this.morphTargetInfluences;if(s&&a){Vo.set(0,0,0);for(let l=0,c=s.length;l<c;l++){let d=a[l],m=s[l];d!==0&&(pc.fromBufferAttribute(m,e),o?Vo.addScaledVector(pc,d):Vo.addScaledVector(pc.sub(t),d))}t.add(Vo)}return t}intersectsFrustum(e){return e.intersectsObject(this)}raycast(e,t){let i=this.geometry,r=this.material,s=this.matrixWorld;r!==void 0&&(i.boundingSphere===null&&i.computeBoundingSphere(),Bo.copy(i.boundingSphere),Bo.applyMatrix4(s),cr.copy(e.ray).recast(e.near),!(Bo.containsPoint(cr.origin)===!1&&(cr.intersectSphere(Bo,gh)===null||cr.origin.distanceToSquared(gh)>(e.far-e.near)**2))&&(mh.copy(s).invert(),cr.copy(e.ray).applyMatrix4(mh),!(i.boundingBox!==null&&cr.intersectsBox(i.boundingBox)===!1)&&this._computeIntersections(e,t,cr)))}_computeIntersections(e,t,i){let r,s=this.geometry,o=this.material,a=s.index,l=s.attributes.position,c=s.attributes.uv,d=s.attributes.uv1,m=s.attributes.normal,u=s.groups,g=s.drawRange;if(a!==null)if(Array.isArray(o))for(let _=0,v=u.length;_<v;_++){let f=u[_],h=o[f.materialIndex],M=Math.max(f.start,g.start),P=Math.min(a.count,Math.min(f.start+f.count,g.start+g.count));for(let w=M,T=P;w<T;w+=3){let E=a.getX(w),U=a.getX(w+1),p=a.getX(w+2);r=Xo(this,h,e,i,c,d,m,E,U,p),r&&(r.faceIndex=Math.floor(w/3),r.face.materialIndex=f.materialIndex,t.push(r))}}else{let _=Math.max(0,g.start),v=Math.min(a.count,g.start+g.count);for(let f=_,h=v;f<h;f+=3){let M=a.getX(f),P=a.getX(f+1),w=a.getX(f+2);r=Xo(this,o,e,i,c,d,m,M,P,w),r&&(r.faceIndex=Math.floor(f/3),t.push(r))}}else if(l!==void 0)if(Array.isArray(o))for(let _=0,v=u.length;_<v;_++){let f=u[_],h=o[f.materialIndex],M=Math.max(f.start,g.start),P=Math.min(l.count,Math.min(f.start+f.count,g.start+g.count));for(let w=M,T=P;w<T;w+=3){let E=w,U=w+1,p=w+2;r=Xo(this,h,e,i,c,d,m,E,U,p),r&&(r.faceIndex=Math.floor(w/3),r.face.materialIndex=f.materialIndex,t.push(r))}}else{let _=Math.max(0,g.start),v=Math.min(l.count,g.start+g.count);for(let f=_,h=v;f<h;f+=3){let M=f,P=f+1,w=f+2;r=Xo(this,o,e,i,c,d,m,M,P,w),r&&(r.faceIndex=Math.floor(f/3),t.push(r))}}}};function Kp(n,e,t,i,r,s,o,a){let l;if(e.side===fn?l=i.intersectTriangle(o,s,r,!0,a):l=i.intersectTriangle(r,s,o,e.side===Ki,a),l===null)return null;Wo.copy(a),Wo.applyMatrix4(n.matrixWorld);let c=t.ray.origin.distanceTo(Wo);return c<t.near||c>t.far?null:{distance:c,point:Wo.clone(),object:n}}function Xo(n,e,t,i,r,s,o,a,l,c){n.getVertexPosition(a,zo),n.getVertexPosition(l,Ho),n.getVertexPosition(c,Go);let d=Kp(n,e,t,i,zo,Ho,Go,xh);if(d){let m=new G;Bi.getBarycoord(xh,zo,Ho,Go,m),r&&(d.uv=Bi.getInterpolatedAttribute(r,a,l,c,m,new We)),s&&(d.uv1=Bi.getInterpolatedAttribute(s,a,l,c,m,new We)),o&&(d.normal=Bi.getInterpolatedAttribute(o,a,l,c,m,new G),d.normal.dot(i.direction)>0&&d.normal.multiplyScalar(-1));let u={a,b:l,c,normal:new G,materialIndex:0};Bi.getNormal(zo,Ho,Go,u.normal),d.face=u,d.barycoord=m}return d}var fr=class extends In{constructor(e=null,t=1,i=1,r,s,o,a,l,c=Qt,d=Qt,m,u){super(null,o,a,l,c,d,r,s,m,u),this.isDataTexture=!0,this.image={data:e,width:t,height:i},this.generateMipmaps=!1,this.flipY=!1,this.unpackAlignment=1}};var Os=class extends Pn{constructor(e,t,i,r=1){super(e,t,i),this.isInstancedBufferAttribute=!0,this.meshPerAttribute=r}copy(e){return super.copy(e),this.meshPerAttribute=e.meshPerAttribute,this}toJSON(){let e=super.toJSON();return e.meshPerAttribute=this.meshPerAttribute,e.isInstancedBufferAttribute=!0,e}},Br=new wt,vh=new wt,qo=[],_h=new Dn,Jp=new wt,Es=new rt,Ts=new Ei,Bs=class extends rt{constructor(e,t,i){super(e,t),this.isInstancedMesh=!0,this.instanceMatrix=new Os(new Float32Array(i*16),16),this.instanceColor=null,this.morphTexture=null,this.count=i,this.boundingBox=null,this.boundingSphere=null;for(let r=0;r<i;r++)this.setMatrixAt(r,Jp)}computeBoundingBox(){let e=this.geometry,t=this.count;this.boundingBox===null&&(this.boundingBox=new Dn),e.boundingBox===null&&e.computeBoundingBox(),this.boundingBox.makeEmpty();for(let i=0;i<t;i++)this.getMatrixAt(i,Br),_h.copy(e.boundingBox).applyMatrix4(Br),this.boundingBox.union(_h)}computeBoundingSphere(){let e=this.geometry,t=this.count;this.boundingSphere===null&&(this.boundingSphere=new Ei),e.boundingSphere===null&&e.computeBoundingSphere(),this.boundingSphere.makeEmpty();for(let i=0;i<t;i++)this.getMatrixAt(i,Br),Ts.copy(e.boundingSphere).applyMatrix4(Br),this.boundingSphere.union(Ts)}copy(e,t){return super.copy(e,t),this.instanceMatrix.copy(e.instanceMatrix),e.morphTexture!==null&&(this.morphTexture=e.morphTexture.clone()),e.instanceColor!==null&&(this.instanceColor=e.instanceColor.clone()),this.count=e.count,e.boundingBox!==null&&(this.boundingBox=e.boundingBox.clone()),e.boundingSphere!==null&&(this.boundingSphere=e.boundingSphere.clone()),this}getColorAt(e,t){return this.instanceColor===null?t.setRGB(1,1,1):t.fromArray(this.instanceColor.array,e*3)}getMatrixAt(e,t){return t.fromArray(this.instanceMatrix.array,e*16)}getMorphAt(e,t){let i=t.morphTargetInfluences,r=this.morphTexture.source.data.data,s=i.length+1,o=e*s+1;for(let a=0;a<i.length;a++)i[a]=r[o+a]}raycast(e,t){let i=this.matrixWorld,r=this.count;if(Es.geometry=this.geometry,Es.material=this.material,Es.material!==void 0&&(this.boundingSphere===null&&this.computeBoundingSphere(),Ts.copy(this.boundingSphere),Ts.applyMatrix4(i),e.ray.intersectsSphere(Ts)!==!1))for(let s=0;s<r;s++){this.getMatrixAt(s,Br),vh.multiplyMatrices(i,Br),Es.matrixWorld=vh,Es.raycast(e,qo);for(let o=0,a=qo.length;o<a;o++){let l=qo[o];l.instanceId=s,l.object=this,t.push(l)}qo.length=0}}setColorAt(e,t){return this.instanceColor===null&&(this.instanceColor=new Os(new Float32Array(this.instanceMatrix.count*3).fill(1),3)),t.toArray(this.instanceColor.array,e*3),this}setMatrixAt(e,t){return t.toArray(this.instanceMatrix.array,e*16),this}setMorphAt(e,t){let i=t.morphTargetInfluences,r=i.length+1;this.morphTexture===null&&(this.morphTexture=new fr(new Float32Array(r*this.count),r,this.count,Da,$n));let s=this.morphTexture.source.data.data,o=0;for(let c=0;c<i.length;c++)o+=i[c];let a=this.geometry.morphTargetsRelative?1:1-o,l=r*e;return s[l]=a,s.set(i,l+1),this}updateMorphTargets(){}dispose(){super.dispose(),this.morphTexture!==null&&(this.morphTexture.dispose(),this.morphTexture=null)}},ur=new Ei,jp=new We(.5,.5),$o=new G,jr=class{constructor(e=new Hn,t=new Hn,i=new Hn,r=new Hn,s=new Hn,o=new Hn){this.planes=[e,t,i,r,s,o]}set(e,t,i,r,s,o){let a=this.planes;return a[0].copy(e),a[1].copy(t),a[2].copy(i),a[3].copy(r),a[4].copy(s),a[5].copy(o),this}copy(e){let t=this.planes;for(let i=0;i<6;i++)t[i].copy(e.planes[i]);return this}setFromProjectionMatrix(e,t=ei,i=!1){let r=this.planes,s=e.elements,o=s[0],a=s[1],l=s[2],c=s[3],d=s[4],m=s[5],u=s[6],g=s[7],_=s[8],v=s[9],f=s[10],h=s[11],M=s[12],P=s[13],w=s[14],T=s[15];if(r[0].setComponents(c-o,g-d,h-_,T-M).normalize(),r[1].setComponents(c+o,g+d,h+_,T+M).normalize(),r[2].setComponents(c+a,g+m,h+v,T+P).normalize(),r[3].setComponents(c-a,g-m,h-v,T-P).normalize(),i)r[4].setComponents(l,u,f,w).normalize(),r[5].setComponents(c-l,g-u,h-f,T-w).normalize();else if(r[4].setComponents(c-l,g-u,h-f,T-w).normalize(),t===ei)r[5].setComponents(c+l,g+u,h+f,T+w).normalize();else if(t===qr)r[5].setComponents(l,u,f,w).normalize();else throw new Error("THREE.Frustum.setFromProjectionMatrix(): Invalid coordinate system: "+t);return this}intersectsObject(e){if(e.boundingSphere!==void 0)e.boundingSphere===null&&e.computeBoundingSphere(),ur.copy(e.boundingSphere).applyMatrix4(e.matrixWorld);else{let t=e.geometry;t.boundingSphere===null&&t.computeBoundingSphere(),ur.copy(t.boundingSphere).applyMatrix4(e.matrixWorld)}return this.intersectsSphere(ur)}intersectsSprite(e){ur.center.set(0,0,0);let t=jp.distanceTo(e.center);return ur.radius=.7071067811865476+t,ur.applyMatrix4(e.matrixWorld),this.intersectsSphere(ur)}intersectsSphere(e){let t=this.planes,i=e.center,r=-e.radius;for(let s=0;s<6;s++)if(t[s].distanceToPoint(i)<r)return!1;return!0}intersectsBox(e){let t=this.planes;for(let i=0;i<6;i++){let r=t[i];if($o.x=r.normal.x>0?e.max.x:e.min.x,$o.y=r.normal.y>0?e.max.y:e.min.y,$o.z=r.normal.z>0?e.max.z:e.min.z,r.distanceToPoint($o)<0)return!1}return!0}containsPoint(e){let t=this.planes;for(let i=0;i<6;i++)if(t[i].distanceToPoint(e)<0)return!1;return!0}clone(){return new this.constructor().copy(this)}};var Qr=class extends Ti{constructor(e){super(),this.isPointsMaterial=!0,this.type="PointsMaterial",this.color=new Ne(16777215),this.map=null,this.alphaMap=null,this.size=1,this.sizeAttenuation=!0,this.fog=!0,this.setValues(e)}copy(e){return super.copy(e),this.color.copy(e.color),this.map=e.map,this.alphaMap=e.alphaMap,this.size=e.size,this.sizeAttenuation=e.sizeAttenuation,this.fog=e.fog,this}},yh=new wt,bc=new Hi,Yo=new Ei,Zo=new G,zs=class extends vn{constructor(e=new $t,t=new Qr){super(),this.isPoints=!0,this.type="Points",this.geometry=e,this.material=t,this.morphTargetDictionary=void 0,this.morphTargetInfluences=void 0,this.updateMorphTargets()}copy(e,t){return super.copy(e,t),this.material=Array.isArray(e.material)?e.material.slice():e.material,this.geometry=e.geometry,this}intersectsFrustum(e){return e.intersectsObject(this)}raycast(e,t){let i=this.geometry,r=this.matrixWorld,s=e.params.Points.threshold,o=i.drawRange;if(i.boundingSphere===null&&i.computeBoundingSphere(),Yo.copy(i.boundingSphere),Yo.applyMatrix4(r),Yo.radius+=s,e.ray.intersectsSphere(Yo)===!1)return;yh.copy(r).invert(),bc.copy(e.ray).applyMatrix4(yh);let a=s/((this.scale.x+this.scale.y+this.scale.z)/3),l=a*a,c=i.index,m=i.attributes.position;if(c!==null){let u=Math.max(0,o.start),g=Math.min(c.count,o.start+o.count);for(let _=u,v=g;_<v;_++){let f=c.getX(_);Zo.fromBufferAttribute(m,f),bh(Zo,f,l,r,e,t,this)}}else{let u=Math.max(0,o.start),g=Math.min(m.count,o.start+o.count);for(let _=u,v=g;_<v;_++)Zo.fromBufferAttribute(m,_),bh(Zo,_,l,r,e,t,this)}}updateMorphTargets(){let t=this.geometry.morphAttributes,i=Object.keys(t);if(i.length>0){let r=t[i[0]];if(r!==void 0){this.morphTargetInfluences=[],this.morphTargetDictionary={};for(let s=0,o=r.length;s<o;s++){let a=r[s].name||String(s);this.morphTargetInfluences.push(0),this.morphTargetDictionary[a]=s}}}}};function bh(n,e,t,i,r,s,o){let a=bc.distanceSqToPoint(n);if(a<t){let l=new G;bc.closestPointToPoint(n,l),l.applyMatrix4(i);let c=r.ray.origin.distanceTo(l);if(c<r.near||c>r.far)return;s.push({distance:c,distanceToRay:Math.sqrt(a),point:l,index:e,face:null,faceIndex:null,barycoord:null,object:o})}}var Hs=class extends In{constructor(e=[],t=Ji,i,r,s,o,a,l,c,d){super(e,t,i,r,s,o,a,l,c,d),this.isCubeTexture=!0,this.flipY=!1}get images(){return this.image}set images(e){this.image=e}},pr=class extends In{constructor(e,t,i,r,s,o,a,l,c){super(e,t,i,r,s,o,a,l,c),this.isCanvasTexture=!0,this.needsUpdate=!0}};var Gi=class extends In{constructor(e,t,i=ii,r,s,o,a=Qt,l=Qt,c,d=hi,m=1){if(d!==hi&&d!==Qi)throw new Error("THREE.DepthTexture: format must be either THREE.DepthFormat or THREE.DepthStencilFormat");let u={width:e,height:t,depth:m};super(u,r,s,o,a,l,d,i,c),this.isDepthTexture=!0,this.flipY=!1,this.generateMipmaps=!1,this.compareFunction=null}copy(e){return super.copy(e),this.source=new Zr(Object.assign({},e.image)),this.compareFunction=e.compareFunction,this}toJSON(e){let t=super.toJSON(e);return t.compareFunction=this.compareFunction,t}},da=class extends Gi{constructor(e,t=ii,i=Ji,r,s,o=Qt,a=Qt,l,c=hi){let d={width:e,height:e,depth:1},m=[d,d,d,d,d,d];super(e,e,t,i,r,s,o,a,l,c),this.image=m,this.isCubeDepthTexture=!0,this.isCubeTexture=!0}get images(){return this.image}set images(e){this.image=e}},Gs=class extends In{constructor(e=null){super(),this.sourceTexture=e,this.isExternalTexture=!0}copy(e){return super.copy(e),this.sourceTexture=e.sourceTexture,this}},Qe=class n extends $t{constructor(e=1,t=1,i=1,r=1,s=1,o=1){super(),this.type="BoxGeometry",this.parameters={width:e,height:t,depth:i,widthSegments:r,heightSegments:s,depthSegments:o};let a=this;r=Math.floor(r),s=Math.floor(s),o=Math.floor(o);let l=[],c=[],d=[],m=[],u=0,g=0;_("z","y","x",-1,-1,i,t,e,o,s,0),_("z","y","x",1,-1,i,t,-e,o,s,1),_("x","z","y",1,1,e,i,t,r,o,2),_("x","z","y",1,-1,e,i,-t,r,o,3),_("x","y","z",1,-1,e,t,i,r,s,4),_("x","y","z",-1,-1,e,t,-i,r,s,5),this.setIndex(l),this.setAttribute("position",new bt(c,3)),this.setAttribute("normal",new bt(d,3)),this.setAttribute("uv",new bt(m,2));function _(v,f,h,M,P,w,T,E,U,p,S){let y=w/U,R=T/p,I=w/2,k=T/2,D=E/2,N=U+1,H=p+1,$=0,ee=0,J=new G;for(let ie=0;ie<H;ie++){let ce=ie*R-k;for(let we=0;we<N;we++){let pe=we*y-I;J[v]=pe*M,J[f]=ce*P,J[h]=D,c.push(J.x,J.y,J.z),J[v]=0,J[f]=0,J[h]=E>0?1:-1,d.push(J.x,J.y,J.z),m.push(we/U),m.push(1-ie/p),$+=1}}for(let ie=0;ie<p;ie++)for(let ce=0;ce<U;ce++){let we=u+ce+N*ie,pe=u+ce+N*(ie+1),Pe=u+(ce+1)+N*(ie+1),Ve=u+(ce+1)+N*ie;l.push(we,pe,Ve),l.push(pe,Pe,Ve),ee+=6}a.addGroup(g,ee,S),g+=ee,u+=$}}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.width,e.height,e.depth,e.widthSegments,e.heightSegments,e.depthSegments)}};var di=class n extends $t{constructor(e=1,t=32,i=0,r=Math.PI*2){super(),this.type="CircleGeometry",this.parameters={radius:e,segments:t,thetaStart:i,thetaLength:r},t=Math.max(3,t);let s=[],o=[],a=[],l=[],c=new G,d=new We;o.push(0,0,0),a.push(0,0,1),l.push(.5,.5);for(let m=0,u=3;m<=t;m++,u+=3){let g=i+m/t*r;c.x=e*Math.cos(g),c.y=e*Math.sin(g),o.push(c.x,c.y,c.z),a.push(0,0,1),d.x=(o[u]/e+1)/2,d.y=(o[u+1]/e+1)/2,l.push(d.x,d.y)}for(let m=1;m<=t;m++)s.push(m,m+1,0);this.setIndex(s),this.setAttribute("position",new bt(o,3)),this.setAttribute("normal",new bt(a,3)),this.setAttribute("uv",new bt(l,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radius,e.segments,e.thetaStart,e.thetaLength)}},zt=class n extends $t{constructor(e=1,t=1,i=1,r=32,s=1,o=!1,a=0,l=Math.PI*2){super(),this.type="CylinderGeometry",this.parameters={radiusTop:e,radiusBottom:t,height:i,radialSegments:r,heightSegments:s,openEnded:o,thetaStart:a,thetaLength:l};let c=this;r=Math.floor(r),s=Math.floor(s);let d=[],m=[],u=[],g=[],_=0,v=[],f=i/2,h=0;M(),o===!1&&(e>0&&P(!0),t>0&&P(!1)),this.setIndex(d),this.setAttribute("position",new bt(m,3)),this.setAttribute("normal",new bt(u,3)),this.setAttribute("uv",new bt(g,2));function M(){let w=new G,T=new G,E=0,U=(t-e)/i;for(let p=0;p<=s;p++){let S=[],y=p/s,R=y*(t-e)+e;for(let I=0;I<=r;I++){let k=I/r,D=k*l+a,N=Math.sin(D),H=Math.cos(D);T.x=R*N,T.y=-y*i+f,T.z=R*H,m.push(T.x,T.y,T.z),w.set(N,U,H).normalize(),u.push(w.x,w.y,w.z),g.push(k,1-y),S.push(_++)}v.push(S)}for(let p=0;p<r;p++)for(let S=0;S<s;S++){let y=v[S][p],R=v[S+1][p],I=v[S+1][p+1],k=v[S][p+1];(e>0||S!==0)&&(d.push(y,R,k),E+=3),(t>0||S!==s-1)&&(d.push(R,I,k),E+=3)}c.addGroup(h,E,0),h+=E}function P(w){let T=_,E=new We,U=new G,p=0,S=w===!0?e:t,y=w===!0?1:-1;for(let I=1;I<=r;I++)m.push(0,f*y,0),u.push(0,y,0),g.push(.5,.5),_++;let R=_;for(let I=0;I<=r;I++){let D=I/r*l+a,N=Math.cos(D),H=Math.sin(D);U.x=S*H,U.y=f*y,U.z=S*N,m.push(U.x,U.y,U.z),u.push(0,y,0),E.x=N*.5+.5,E.y=H*.5*y+.5,g.push(E.x,E.y),_++}for(let I=0;I<r;I++){let k=T+I,D=R+I;w===!0?d.push(D,D+1,k):d.push(D+1,D,k),p+=3}c.addGroup(h,p,w===!0?1:2),h+=p}}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radiusTop,e.radiusBottom,e.height,e.radialSegments,e.heightSegments,e.openEnded,e.thetaStart,e.thetaLength)}},es=class n extends zt{constructor(e=1,t=1,i=32,r=1,s=!1,o=0,a=Math.PI*2){super(0,e,t,i,r,s,o,a),this.type="ConeGeometry",this.parameters={radius:e,height:t,radialSegments:i,heightSegments:r,openEnded:s,thetaStart:o,thetaLength:a}}static fromJSON(e){return new n(e.radius,e.height,e.radialSegments,e.heightSegments,e.openEnded,e.thetaStart,e.thetaLength)}},Vs=class n extends $t{constructor(e=[],t=[],i=1,r=0){super(),this.type="PolyhedronGeometry",this.parameters={vertices:e,indices:t,radius:i,detail:r};let s=[],o=[];a(r),c(i),d(),this.setAttribute("position",new bt(s,3)),this.setAttribute("normal",new bt(s.slice(),3)),this.setAttribute("uv",new bt(o,2)),r===0?this.computeVertexNormals():this.normalizeNormals();function a(M){let P=new G,w=new G,T=new G;for(let E=0;E<t.length;E+=3)g(t[E+0],P),g(t[E+1],w),g(t[E+2],T),l(P,w,T,M)}function l(M,P,w,T){let E=T+1,U=[];for(let p=0;p<=E;p++){U[p]=[];let S=M.clone().lerp(w,p/E),y=P.clone().lerp(w,p/E),R=E-p;for(let I=0;I<=R;I++)I===0&&p===E?U[p][I]=S:U[p][I]=S.clone().lerp(y,I/R)}for(let p=0;p<E;p++)for(let S=0;S<2*(E-p)-1;S++){let y=Math.floor(S/2);S%2===0?(u(U[p][y+1]),u(U[p+1][y]),u(U[p][y])):(u(U[p][y+1]),u(U[p+1][y+1]),u(U[p+1][y]))}}function c(M){let P=new G;for(let w=0;w<s.length;w+=3)P.x=s[w+0],P.y=s[w+1],P.z=s[w+2],P.normalize().multiplyScalar(M),s[w+0]=P.x,s[w+1]=P.y,s[w+2]=P.z}function d(){let M=new G;for(let P=0;P<s.length;P+=3){M.x=s[P+0],M.y=s[P+1],M.z=s[P+2];let w=f(M)/2/Math.PI+.5,T=h(M)/Math.PI+.5;o.push(w,1-T)}_(),m()}function m(){for(let M=0;M<o.length;M+=6){let P=o[M+0],w=o[M+2],T=o[M+4],E=Math.max(P,w,T),U=Math.min(P,w,T);E>.9&&U<.1&&(P<.2&&(o[M+0]+=1),w<.2&&(o[M+2]+=1),T<.2&&(o[M+4]+=1))}}function u(M){s.push(M.x,M.y,M.z)}function g(M,P){let w=M*3;P.x=e[w+0],P.y=e[w+1],P.z=e[w+2]}function _(){let M=new G,P=new G,w=new G,T=new G,E=new We,U=new We,p=new We;for(let S=0,y=0;S<s.length;S+=9,y+=6){M.set(s[S+0],s[S+1],s[S+2]),P.set(s[S+3],s[S+4],s[S+5]),w.set(s[S+6],s[S+7],s[S+8]),E.set(o[y+0],o[y+1]),U.set(o[y+2],o[y+3]),p.set(o[y+4],o[y+5]),T.copy(M).add(P).add(w).divideScalar(3);let R=f(T);v(E,y+0,M,R),v(U,y+2,P,R),v(p,y+4,w,R)}}function v(M,P,w,T){T<0&&M.x===1&&(o[P]=M.x-1),w.x===0&&w.z===0&&(o[P]=T/2/Math.PI+.5)}function f(M){return Math.atan2(M.z,-M.x)}function h(M){return Math.atan2(-M.y,Math.sqrt(M.x*M.x+M.z*M.z))}}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.vertices,e.indices,e.radius,e.detail)}},Ws=class n extends Vs{constructor(e=1,t=0){let i=(1+Math.sqrt(5))/2,r=1/i,s=[-1,-1,-1,-1,-1,1,-1,1,-1,-1,1,1,1,-1,-1,1,-1,1,1,1,-1,1,1,1,0,-r,-i,0,-r,i,0,r,-i,0,r,i,-r,-i,0,-r,i,0,r,-i,0,r,i,0,-i,0,-r,i,0,-r,-i,0,r,i,0,r],o=[3,11,7,3,7,15,3,15,13,7,19,17,7,17,6,7,6,15,17,4,8,17,8,10,17,10,6,8,0,16,8,16,2,8,2,10,0,12,1,0,1,18,0,18,16,6,10,2,6,2,13,6,13,15,2,16,18,2,18,3,2,3,13,18,1,9,18,9,11,18,11,3,4,14,12,4,12,0,4,0,8,11,9,5,11,5,19,11,19,7,19,5,14,19,14,4,19,4,17,1,12,14,1,14,5,1,5,9];super(s,o,e,t),this.type="DodecahedronGeometry",this.parameters={radius:e,detail:t}}static fromJSON(e){return new n(e.radius,e.detail)}};var Xs=class n extends Vs{constructor(e=1,t=0){let i=[1,0,0,-1,0,0,0,1,0,0,-1,0,0,0,1,0,0,-1],r=[0,2,4,0,4,3,0,3,5,0,5,2,1,2,5,1,5,3,1,3,4,1,4,2];super(i,r,e,t),this.type="OctahedronGeometry",this.parameters={radius:e,detail:t}}static fromJSON(e){return new n(e.radius,e.detail)}},Vi=class n extends $t{constructor(e=1,t=1,i=1,r=1){super(),this.type="PlaneGeometry",this.parameters={width:e,height:t,widthSegments:i,heightSegments:r};let s=e/2,o=t/2,a=Math.floor(i),l=Math.floor(r),c=a+1,d=l+1,m=e/a,u=t/l,g=[],_=[],v=[],f=[];for(let h=0;h<d;h++){let M=h*u-o;for(let P=0;P<c;P++){let w=P*m-s;_.push(w,-M,0),v.push(0,0,1),f.push(P/a),f.push(1-h/l)}}for(let h=0;h<l;h++)for(let M=0;M<a;M++){let P=M+c*h,w=M+c*(h+1),T=M+1+c*(h+1),E=M+1+c*h;g.push(P,w,E),g.push(w,T,E)}this.setIndex(g),this.setAttribute("position",new bt(_,3)),this.setAttribute("normal",new bt(v,3)),this.setAttribute("uv",new bt(f,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.width,e.height,e.widthSegments,e.heightSegments)}},qs=class n extends $t{constructor(e=.5,t=1,i=32,r=1,s=0,o=Math.PI*2){super(),this.type="RingGeometry",this.parameters={innerRadius:e,outerRadius:t,thetaSegments:i,phiSegments:r,thetaStart:s,thetaLength:o},i=Math.max(3,i),r=Math.max(1,r);let a=[],l=[],c=[],d=[],m=e,u=(t-e)/r,g=new G,_=new We;for(let v=0;v<=r;v++){for(let f=0;f<=i;f++){let h=s+f/i*o;g.x=m*Math.cos(h),g.y=m*Math.sin(h),l.push(g.x,g.y,g.z),c.push(0,0,1),_.x=(g.x/t+1)/2,_.y=(g.y/t+1)/2,d.push(_.x,_.y)}m+=u}for(let v=0;v<r;v++){let f=v*(i+1);for(let h=0;h<i;h++){let M=h+f,P=M,w=M+i+1,T=M+i+2,E=M+1;a.push(P,w,E),a.push(w,T,E)}}this.setIndex(a),this.setAttribute("position",new bt(l,3)),this.setAttribute("normal",new bt(c,3)),this.setAttribute("uv",new bt(d,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.innerRadius,e.outerRadius,e.thetaSegments,e.phiSegments,e.thetaStart,e.thetaLength)}};var Vt=class n extends $t{constructor(e=1,t=32,i=16,r=0,s=Math.PI*2,o=0,a=Math.PI){super(),this.type="SphereGeometry",this.parameters={radius:e,widthSegments:t,heightSegments:i,phiStart:r,phiLength:s,thetaStart:o,thetaLength:a},t=Math.max(3,Math.floor(t)),i=Math.max(2,Math.floor(i));let l=Math.min(o+a,Math.PI),c=0,d=[],m=new G,u=new G,g=[],_=[],v=[],f=[];for(let h=0;h<=i;h++){let M=[],P=h/i,w=o+P*a,T=e*Math.cos(w),E=Math.sqrt(e*e-T*T),U=0;h===0&&o===0?U=.5/t:h===i&&l===Math.PI&&(U=-.5/t);for(let p=0;p<=t;p++){let S=p/t,y=r+S*s;m.x=-E*Math.cos(y),m.y=T,m.z=E*Math.sin(y),_.push(m.x,m.y,m.z),u.copy(m).normalize(),v.push(u.x,u.y,u.z),f.push(S+U,1-P),M.push(c++)}d.push(M)}for(let h=0;h<i;h++)for(let M=0;M<t;M++){let P=d[h][M+1],w=d[h][M],T=d[h+1][M],E=d[h+1][M+1];(h!==0||o>0)&&g.push(P,w,E),(h!==i-1||l<Math.PI)&&g.push(w,T,E)}this.setIndex(g),this.setAttribute("position",new bt(_,3)),this.setAttribute("normal",new bt(v,3)),this.setAttribute("uv",new bt(f,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radius,e.widthSegments,e.heightSegments,e.phiStart,e.phiLength,e.thetaStart,e.thetaLength)}};var Wi=class n extends $t{constructor(e=1,t=.4,i=12,r=48,s=Math.PI*2,o=0,a=Math.PI*2){super(),this.type="TorusGeometry",this.parameters={radius:e,tube:t,radialSegments:i,tubularSegments:r,arc:s,thetaStart:o,thetaLength:a},i=Math.floor(i),r=Math.floor(r);let l=[],c=[],d=[],m=[],u=new G,g=new G,_=new G;for(let v=0;v<=i;v++){let f=o+v/i*a;for(let h=0;h<=r;h++){let M=h/r*s;g.x=(e+t*Math.cos(f))*Math.cos(M),g.y=(e+t*Math.cos(f))*Math.sin(M),g.z=t*Math.sin(f),c.push(g.x,g.y,g.z),u.x=e*Math.cos(M),u.y=e*Math.sin(M),_.subVectors(g,u).normalize(),d.push(_.x,_.y,_.z),m.push(h/r),m.push(v/i)}}for(let v=1;v<=i;v++)for(let f=1;f<=r;f++){let h=(r+1)*v+f-1,M=(r+1)*(v-1)+f-1,P=(r+1)*(v-1)+f,w=(r+1)*v+f;l.push(h,M,w),l.push(M,P,w)}this.setIndex(l),this.setAttribute("position",new bt(c,3)),this.setAttribute("normal",new bt(d,3)),this.setAttribute("uv",new bt(m,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radius,e.tube,e.radialSegments,e.tubularSegments,e.arc,e.thetaStart,e.thetaLength)}};function vr(n){let e={};for(let t in n){e[t]={};for(let i in n[t]){let r=n[t][i];if(Mh(r))r.isRenderTargetTexture?($e("UniformsUtils: Textures of render targets cannot be cloned via cloneUniforms() or mergeUniforms()."),e[t][i]=null):e[t][i]=r.clone();else if(Array.isArray(r))if(Mh(r[0])){let s=[];for(let o=0,a=r.length;o<a;o++)s[o]=r[o].clone();e[t][i]=s}else e[t][i]=r.slice();else e[t][i]=r}}return e}function An(n){let e={};for(let t=0;t<n.length;t++){let i=vr(n[t]);for(let r in i)e[r]=i[r]}return e}function Mh(n){return n&&(n.isColor||n.isMatrix3||n.isMatrix4||n.isVector2||n.isVector3||n.isVector4||n.isTexture||n.isQuaternion)}function Qp(n){let e=[];for(let t=0;t<n.length;t++)e.push(n[t].clone());return e}function Jc(n){let e=n.getRenderTarget();return e===null?n.outputColorSpace:e.isXRRenderTarget===!0?e.texture.colorSpace:ut.workingColorSpace}var xl={clone:vr,merge:An},em=`void main() {
	gl_Position = projectionMatrix * modelViewMatrix * vec4( position, 1.0 );
}`,tm=`void main() {
	gl_FragColor = vec4( 1.0, 0.0, 0.0, 1.0 );
}`,Tn=class extends Ti{constructor(e){super(),this.isShaderMaterial=!0,this.type="ShaderMaterial",this.defines={},this.uniforms={},this.uniformsGroups=[],this.vertexShader=em,this.fragmentShader=tm,this.linewidth=1,this.wireframe=!1,this.wireframeLinewidth=1,this.fog=!1,this.lights=!1,this.clipping=!1,this.forceSinglePass=!0,this.extensions={clipCullDistance:!1,multiDraw:!1},this.defaultAttributeValues={color:[1,1,1],uv:[0,0],uv1:[0,0]},this.index0AttributeName=void 0,this.uniformsNeedUpdate=!1,this.glslVersion=null,e!==void 0&&this.setValues(e)}copy(e){return super.copy(e),this.fragmentShader=e.fragmentShader,this.vertexShader=e.vertexShader,this.uniforms=vr(e.uniforms),this.uniformsGroups=Qp(e.uniformsGroups),this.defines=Object.assign({},e.defines),this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this.fog=e.fog,this.lights=e.lights,this.clipping=e.clipping,this.extensions=Object.assign({},e.extensions),this.glslVersion=e.glslVersion,this.defaultAttributeValues=Object.assign({},e.defaultAttributeValues),this.index0AttributeName=e.index0AttributeName,this.uniformsNeedUpdate=e.uniformsNeedUpdate,this}toJSON(e){let t=super.toJSON(e);t.glslVersion=this.glslVersion,t.uniforms={};for(let r in this.uniforms){let o=this.uniforms[r].value;o&&o.isTexture?t.uniforms[r]={type:"t",value:o.toJSON(e).uuid}:o&&o.isColor?t.uniforms[r]={type:"c",value:o.getHex()}:o&&o.isVector2?t.uniforms[r]={type:"v2",value:o.toArray()}:o&&o.isVector3?t.uniforms[r]={type:"v3",value:o.toArray()}:o&&o.isVector4?t.uniforms[r]={type:"v4",value:o.toArray()}:o&&o.isMatrix3?t.uniforms[r]={type:"m3",value:o.toArray()}:o&&o.isMatrix4?t.uniforms[r]={type:"m4",value:o.toArray()}:t.uniforms[r]={value:o}}Object.keys(this.defines).length>0&&(t.defines=this.defines),t.vertexShader=this.vertexShader,t.fragmentShader=this.fragmentShader,t.lights=this.lights,t.clipping=this.clipping;let i={};for(let r in this.extensions)this.extensions[r]===!0&&(i[r]=!0);return Object.keys(i).length>0&&(t.extensions=i),t}fromJSON(e,t){if(super.fromJSON(e,t),e.uniforms!==void 0)for(let i in e.uniforms){let r=e.uniforms[i];switch(this.uniforms[i]={},r.type){case"t":this.uniforms[i].value=t[r.value]||null;break;case"c":this.uniforms[i].value=new Ne().setHex(r.value);break;case"v2":this.uniforms[i].value=new We().fromArray(r.value);break;case"v3":this.uniforms[i].value=new G().fromArray(r.value);break;case"v4":this.uniforms[i].value=new Bt().fromArray(r.value);break;case"m3":this.uniforms[i].value=new je().fromArray(r.value);break;case"m4":this.uniforms[i].value=new wt().fromArray(r.value);break;default:this.uniforms[i].value=r.value}}if(e.defines!==void 0&&(this.defines=e.defines),e.vertexShader!==void 0&&(this.vertexShader=e.vertexShader),e.fragmentShader!==void 0&&(this.fragmentShader=e.fragmentShader),e.glslVersion!==void 0&&(this.glslVersion=e.glslVersion),e.extensions!==void 0)for(let i in e.extensions)this.extensions[i]=e.extensions[i];return e.lights!==void 0&&(this.lights=e.lights),e.clipping!==void 0&&(this.clipping=e.clipping),this}},fa=class extends Tn{constructor(e){super(e),this.isRawShaderMaterial=!0,this.type="RawShaderMaterial"}};var $s=class extends Ti{constructor(e){super(),this.isMeshToonMaterial=!0,this.defines={TOON:""},this.type="MeshToonMaterial",this.color=new Ne(16777215),this.map=null,this.gradientMap=null,this.lightMap=null,this.lightMapIntensity=1,this.aoMap=null,this.aoMapIntensity=1,this.emissive=new Ne(0),this.emissiveIntensity=1,this.emissiveMap=null,this.bumpMap=null,this.bumpScale=1,this.normalMap=null,this.normalMapType=pl,this.normalScale=new We(1,1),this.displacementMap=null,this.displacementScale=1,this.displacementBias=0,this.alphaMap=null,this.wireframe=!1,this.wireframeLinewidth=1,this.wireframeLinecap="round",this.wireframeLinejoin="round",this.fog=!0,this.setValues(e)}copy(e){return super.copy(e),this.color.copy(e.color),this.map=e.map,this.gradientMap=e.gradientMap,this.lightMap=e.lightMap,this.lightMapIntensity=e.lightMapIntensity,this.aoMap=e.aoMap,this.aoMapIntensity=e.aoMapIntensity,this.emissive.copy(e.emissive),this.emissiveMap=e.emissiveMap,this.emissiveIntensity=e.emissiveIntensity,this.bumpMap=e.bumpMap,this.bumpScale=e.bumpScale,this.normalMap=e.normalMap,this.normalMapType=e.normalMapType,this.normalScale.copy(e.normalScale),this.displacementMap=e.displacementMap,this.displacementScale=e.displacementScale,this.displacementBias=e.displacementBias,this.alphaMap=e.alphaMap,this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this.wireframeLinecap=e.wireframeLinecap,this.wireframeLinejoin=e.wireframeLinejoin,this.fog=e.fog,this}};var pa=class extends Ti{constructor(e){super(),this.isMeshDepthMaterial=!0,this.type="MeshDepthMaterial",this.depthPacking=ed,this.map=null,this.alphaMap=null,this.displacementMap=null,this.displacementScale=1,this.displacementBias=0,this.wireframe=!1,this.wireframeLinewidth=1,this.setValues(e)}copy(e){return super.copy(e),this.depthPacking=e.depthPacking,this.map=e.map,this.alphaMap=e.alphaMap,this.displacementMap=e.displacementMap,this.displacementScale=e.displacementScale,this.displacementBias=e.displacementBias,this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this}},ma=class extends Ti{constructor(e){super(),this.isMeshDistanceMaterial=!0,this.type="MeshDistanceMaterial",this.map=null,this.alphaMap=null,this.displacementMap=null,this.displacementScale=1,this.displacementBias=0,this.setValues(e)}copy(e){return super.copy(e),this.map=e.map,this.alphaMap=e.alphaMap,this.displacementMap=e.displacementMap,this.displacementScale=e.displacementScale,this.displacementBias=e.displacementBias,this}};function zr(n,e){return!n||n.constructor===e?n:typeof e.BYTES_PER_ELEMENT=="number"?new e(n):Array.prototype.slice.call(n)}function mc(n){return n!==void 0&&n.inTangents!==void 0&&n.outTangents!==void 0}var Xi=class{constructor(e,t,i,r){this.parameterPositions=e,this._cachedIndex=0,this.resultBuffer=r!==void 0?r:new t.constructor(i),this.sampleValues=t,this.valueSize=i,this.settings=null,this.DefaultSettings_={}}evaluate(e){let t=this.parameterPositions,i=this._cachedIndex,r=t[i],s=t[i-1];n:{e:{let o;t:{i:if(!(e<r)){for(let a=i+2;;){if(r===void 0){if(e<s)break i;return i=t.length,this._cachedIndex=i,this.copySampleValue_(i-1)}if(i===a)break;if(s=r,r=t[++i],e<r)break e}o=t.length;break t}if(!(e>=s)){let a=t[1];e<a&&(i=2,s=a);for(let l=i-2;;){if(s===void 0)return this._cachedIndex=0,this.copySampleValue_(0);if(i===l)break;if(r=s,s=t[--i-1],e>=s)break e}o=i,i=0;break t}break n}for(;i<o;){let a=i+o>>>1;e<t[a]?o=a:i=a+1}if(r=t[i],s=t[i-1],s===void 0)return this._cachedIndex=0,this.copySampleValue_(0);if(r===void 0)return i=t.length,this._cachedIndex=i,this.copySampleValue_(i-1)}this._cachedIndex=i,this.intervalChanged_(i,s,r)}return this.interpolate_(i,s,e,r)}getSettings_(){return this.settings||this.DefaultSettings_}copySampleValue_(e){let t=this.resultBuffer,i=this.sampleValues,r=this.valueSize,s=e*r;for(let o=0;o!==r;++o)t[o]=i[s+o];return t}interpolate_(){throw new Error("THREE.Interpolant: Call to abstract method.")}intervalChanged_(){}},ga=class extends Xi{constructor(e,t,i,r){super(e,t,i,r),this._weightPrev=-0,this._offsetPrev=-0,this._weightNext=-0,this._offsetNext=-0,this.DefaultSettings_={endingStart:vc,endingEnd:vc}}intervalChanged_(e,t,i){let r=this.parameterPositions,s=e-2,o=e+1,a=r[s],l=r[o];if(a===void 0)switch(this.getSettings_().endingStart){case _c:s=e,a=2*t-i;break;case yc:s=r.length-2,a=t+r[s]-r[s+1];break;default:s=e,a=i}if(l===void 0)switch(this.getSettings_().endingEnd){case _c:o=e,l=2*i-t;break;case yc:o=1,l=i+r[1]-r[0];break;default:o=e-1,l=t}let c=(i-t)*.5,d=this.valueSize;this._weightPrev=c/(t-a),this._weightNext=c/(l-i),this._offsetPrev=s*d,this._offsetNext=o*d}interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,l=e*a,c=l-a,d=this._offsetPrev,m=this._offsetNext,u=this._weightPrev,g=this._weightNext,_=(i-t)/(r-t),v=_*_,f=v*_,h=-u*f+2*u*v-u*_,M=(1+u)*f+(-1.5-2*u)*v+(-.5+u)*_+1,P=(-1-g)*f+(1.5+g)*v+.5*_,w=g*f-g*v;for(let T=0;T!==a;++T)s[T]=h*o[d+T]+M*o[c+T]+P*o[l+T]+w*o[m+T];return s}},xa=class extends Xi{constructor(e,t,i,r){super(e,t,i,r)}interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,l=e*a,c=l-a,d=(i-t)/(r-t),m=1-d;for(let u=0;u!==a;++u)s[u]=o[c+u]*m+o[l+u]*d;return s}},va=class extends Xi{constructor(e,t,i,r){super(e,t,i,r)}interpolate_(e){return this.copySampleValue_(e-1)}},_a=class extends Xi{interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,l=e*a,c=l-a,d=this.inTangents,m=this.outTangents;if(!d||!m){let _=(i-t)/(r-t),v=1-_;for(let f=0;f!==a;++f)s[f]=o[c+f]*v+o[l+f]*_;return s}let u=a*2,g=e-1;for(let _=0;_!==a;++_){let v=o[c+_],f=o[l+_],h=g*u+_*2,M=m[h],P=m[h+1],w=e*u+_*2,T=d[w],E=d[w+1],U=im(i,t,M,T,r);s[_]=md(U,v,P,E,f)}return s}};function md(n,e,t,i,r){let s=1-n;return s*s*s*e+3*s*s*n*t+3*s*n*n*i+n*n*n*r}function nm(n,e,t,i,r){let s=1-n;return 3*s*s*(t-e)+6*s*n*(i-t)+3*n*n*(r-i)}function im(n,e,t,i,r){let s=(n-e)/(r-e);for(let o=0;o<8;o++){let a=md(s,e,t,i,r)-n;if(Math.abs(a)<1e-10)break;let l=nm(s,e,t,i,r);if(Math.abs(l)<1e-10)break;s=Math.max(0,Math.min(1,s-a/l))}return s}var Gn=class{constructor(e,t,i,r){if(e===void 0)throw new Error("THREE.KeyframeTrack: track name is undefined");if(t===void 0||t.length===0)throw new Error("THREE.KeyframeTrack: no keyframes in track named "+e);this.name=e,this.times=zr(t,this.TimeBufferType),this.values=zr(i,this.ValueBufferType),this.setInterpolation(r||this.DefaultInterpolation)}static toJSON(e){let t=e.constructor,i;if(t.toJSON!==this.toJSON)i=t.toJSON(e);else{i={name:e.name,times:zr(e.times,Array),values:zr(e.values,Array)};let r=e.getInterpolation();r!==e.DefaultInterpolation&&(i.interpolation=r),mc(e.settings)&&(i.settings={inTangents:zr(e.settings.inTangents,Array),outTangents:zr(e.settings.outTangents,Array)})}return i.type=e.ValueTypeName,i}InterpolantFactoryMethodDiscrete(e){return new va(this.times,this.values,this.getValueSize(),e)}InterpolantFactoryMethodLinear(e){return new xa(this.times,this.values,this.getValueSize(),e)}InterpolantFactoryMethodSmooth(e){return new ga(this.times,this.values,this.getValueSize(),e)}InterpolantFactoryMethodBezier(e){let t=new _a(this.times,this.values,this.getValueSize(),e);return this.settings&&(t.inTangents=this.settings.inTangents,t.outTangents=this.settings.outTangents),t}setInterpolation(e){let t;switch(e){case Cs:t=this.InterpolantFactoryMethodDiscrete;break;case la:t=this.InterpolantFactoryMethodLinear;break;case jo:t=this.InterpolantFactoryMethodSmooth;break;case xc:t=this.InterpolantFactoryMethodBezier;break}if(t===void 0){let i="unsupported interpolation for "+this.ValueTypeName+" keyframe track named "+this.name;if(this.createInterpolant===void 0)if(e!==this.DefaultInterpolation)this.setInterpolation(this.DefaultInterpolation);else throw new Error(i);return $e("KeyframeTrack:",i),this}return this.createInterpolant=t,this}getInterpolation(){switch(this.createInterpolant){case this.InterpolantFactoryMethodDiscrete:return Cs;case this.InterpolantFactoryMethodLinear:return la;case this.InterpolantFactoryMethodSmooth:return jo;case this.InterpolantFactoryMethodBezier:return xc}}getValueSize(){return this.values.length/this.times.length}shift(e){if(e!==0){let t=this.times;for(let i=0,r=t.length;i!==r;++i)t[i]+=e}return this}scale(e){if(e!==1){let t=this.times;for(let i=0,r=t.length;i!==r;++i)t[i]*=e;mc(this.settings)&&(Sh(this.settings.inTangents,e),Sh(this.settings.outTangents,e))}return this}trim(e,t){let i=this.times,r=i.length,s=0,o=r-1;for(;s!==r&&i[s]<e;)++s;for(;o!==-1&&i[o]>t;)--o;if(++o,s!==0||o!==r){s>=o&&(o=Math.max(o,1),s=o-1);let a=this.getValueSize();this.times=i.slice(s,o),this.values=this.values.slice(s*a,o*a)}return this}validate(){let e=!0,t=this.getValueSize();t-Math.floor(t)!==0&&(Ze("KeyframeTrack: Invalid value size in track.",this),e=!1);let i=this.times,r=this.values,s=i.length;s===0&&(Ze("KeyframeTrack: Track is empty.",this),e=!1);let o=null;for(let a=0;a!==s;a++){let l=i[a];if(typeof l=="number"&&isNaN(l)){Ze("KeyframeTrack: Time is not a valid number.",this,a,l),e=!1;break}if(o!==null&&o>l){Ze("KeyframeTrack: Out of order keys.",this,a,l,o),e=!1;break}o=l}if(r!==void 0&&xp(r))for(let a=0,l=r.length;a!==l;++a){let c=r[a];if(isNaN(c)){Ze("KeyframeTrack: Value is not a valid number.",this,a,c),e=!1;break}}return e}optimize(){let e=this.times.slice(),t=this.values.slice(),i=this.getValueSize(),r=this.getInterpolation()===jo,s=e.length-1,o=1;for(let a=1;a<s;++a){let l=!1,c=e[a],d=e[a+1];if(c!==d&&(a!==1||c!==e[0]))if(r)l=!0;else{let m=a*i,u=m-i,g=m+i;for(let _=0;_!==i;++_){let v=t[m+_];if(v!==t[u+_]||v!==t[g+_]){l=!0;break}}}if(l){if(a!==o){e[o]=e[a];let m=a*i,u=o*i;for(let g=0;g!==i;++g)t[u+g]=t[m+g]}++o}}if(s>0){e[o]=e[s];for(let a=s*i,l=o*i,c=0;c!==i;++c)t[l+c]=t[a+c];++o}return o!==e.length?(this.times=e.slice(0,o),this.values=t.slice(0,o*i)):(this.times=e,this.values=t),this}clone(){let e=this.times.slice(),t=this.values.slice(),i=this.constructor,r=new i(this.name,e,t);return r.createInterpolant=this.createInterpolant,mc(this.settings)&&(r.settings={inTangents:this.settings.inTangents.slice(),outTangents:this.settings.outTangents.slice()}),r}};function Sh(n,e){for(let t=0,i=n.length;t!==i;t+=2)n[t]*=e}Gn.prototype.ValueTypeName="";Gn.prototype.TimeBufferType=Float32Array;Gn.prototype.ValueBufferType=Float32Array;Gn.prototype.DefaultInterpolation=la;var qi=class extends Gn{constructor(e,t,i){super(e,t,i)}};qi.prototype.ValueTypeName="bool";qi.prototype.ValueBufferType=Array;qi.prototype.DefaultInterpolation=Cs;qi.prototype.InterpolantFactoryMethodLinear=void 0;qi.prototype.InterpolantFactoryMethodSmooth=void 0;var ya=class extends Gn{constructor(e,t,i,r){super(e,t,i,r)}};ya.prototype.ValueTypeName="color";var ba=class extends Gn{constructor(e,t,i,r){super(e,t,i,r)}};ba.prototype.ValueTypeName="number";var Ma=class extends Xi{constructor(e,t,i,r){super(e,t,i,r)}interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,l=(i-t)/(r-t),c=e*a;for(let d=c+a;c!==d;c+=4)an.slerpFlat(s,0,o,c-a,o,c,l);return s}},Ys=class extends Gn{constructor(e,t,i,r){super(e,t,i,r)}InterpolantFactoryMethodLinear(e){return new Ma(this.times,this.values,this.getValueSize(),e)}};Ys.prototype.ValueTypeName="quaternion";Ys.prototype.InterpolantFactoryMethodSmooth=void 0;var $i=class extends Gn{constructor(e,t,i){super(e,t,i)}};$i.prototype.ValueTypeName="string";$i.prototype.ValueBufferType=Array;$i.prototype.DefaultInterpolation=Cs;$i.prototype.InterpolantFactoryMethodLinear=void 0;$i.prototype.InterpolantFactoryMethodSmooth=void 0;var Sa=class extends Gn{constructor(e,t,i,r){super(e,t,i,r)}};Sa.prototype.ValueTypeName="vector";var wa=class{constructor(e,t,i){let r=this,s=!1,o=0,a=0,l,c=[];this.onStart=void 0,this.onLoad=e,this.onProgress=t,this.onError=i,this._abortController=null,this.itemStart=function(d){a++,s===!1&&r.onStart!==void 0&&r.onStart(d,o,a),s=!0},this.itemEnd=function(d){o++,r.onProgress!==void 0&&r.onProgress(d,o,a),o===a&&(s=!1,r.onLoad!==void 0&&r.onLoad())},this.itemError=function(d){r.onError!==void 0&&r.onError(d)},this.resolveURL=function(d){return d=d.normalize("NFC"),l?l(d):d},this.setURLModifier=function(d){return l=d,this},this.addHandler=function(d,m){return c.push(d,m),this},this.removeHandler=function(d){let m=c.indexOf(d);return m!==-1&&c.splice(m,2),this},this.getHandler=function(d){for(let m=0,u=c.length;m<u;m+=2){let g=c[m],_=c[m+1];if(g.global&&(g.lastIndex=0),g.test(d))return _}return null},this.abort=function(){return this.abortController.abort(),this._abortController=null,this}}get abortController(){return this._abortController||(this._abortController=new AbortController),this._abortController}},gd=new wa,Ea=class{constructor(e){this.manager=e!==void 0?e:gd,this.crossOrigin="anonymous",this.withCredentials=!1,this.path="",this.resourcePath="",this.requestHeader={},typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("observe",{detail:this}))}load(){}loadAsync(e,t){let i=this;return new Promise(function(r,s){i.load(e,r,t,s)})}parse(){}setCrossOrigin(e){return this.crossOrigin=e,this}setWithCredentials(e){return this.withCredentials=e,this}setPath(e){return this.path=e,this}setResourcePath(e){return this.resourcePath=e,this}setRequestHeader(e){return this.requestHeader=e,this}abort(){return this}};Ea.DEFAULT_MATERIAL_NAME="__DEFAULT";var ts=class extends vn{constructor(e,t=1){super(),this.isLight=!0,this.type="Light",this.color=new Ne(e),this.intensity=t}copy(e,t){return super.copy(e,t),this.color.copy(e.color),this.intensity=e.intensity,this}toJSON(e){let t=super.toJSON(e);return t.object.color=this.color.getHex(),t.object.intensity=this.intensity,t}},Zs=class extends ts{constructor(e,t,i){super(e,i),this.isHemisphereLight=!0,this.type="HemisphereLight",this.position.copy(vn.DEFAULT_UP),this.updateMatrix(),this.groundColor=new Ne(t)}copy(e,t){return super.copy(e,t),this.groundColor.copy(e.groundColor),this}toJSON(e){let t=super.toJSON(e);return t.object.groundColor=this.groundColor.getHex(),t}},gc=new wt,wh=new G,Eh=new G,Ks=class{constructor(e){this.camera=e,this.intensity=1,this.bias=0,this.biasNode=null,this.normalBias=0,this.radius=1,this.blurSamples=8,this.mapSize=new We(512,512),this.mapType=Un,this.map=null,this.mapPass=null,this.matrix=new wt,this.autoUpdate=!0,this.needsUpdate=!1,this._frustum=new jr,this._frameExtents=new We(1,1),this._viewportCount=1,this._viewports=[new Bt(0,0,1,1)]}getViewportCount(){return this._viewportCount}getCamera(){return this.camera}getFrustum(){return this._frustum}updateMatrices(e){let t=this.camera;wh.setFromMatrixPosition(e.matrixWorld),t.position.copy(wh),Eh.setFromMatrixPosition(e.target.matrixWorld),t.lookAt(Eh),t.updateMatrixWorld(),this._updateMatrix(t,this.matrix,this._frustum)}_updateMatrix(e,t,i,r){gc.multiplyMatrices(e.projectionMatrix,e.matrixWorldInverse),i.setFromProjectionMatrix(gc,e.coordinateSystem,e.reversedDepth);let s=this._frameExtents,o=r?r.z/s.x:1,a=r?r.w/s.y:1,l=r?r.x/s.x:0,c=r?r.y/s.y:0;e.coordinateSystem===qr||e.reversedDepth?t.set(.5*o,0,0,.5*o+l,0,.5*a,0,.5*a+c,0,0,1,0,0,0,0,1):t.set(.5*o,0,0,.5*o+l,0,.5*a,0,.5*a+c,0,0,.5,.5,0,0,0,1),t.multiply(gc)}getViewport(e){return this._viewports[e]}getFrameExtents(){return this._frameExtents}dispose(){this.map&&this.map.dispose(),this.mapPass&&this.mapPass.dispose()}copy(e){return this.camera=e.camera.clone(),this.intensity=e.intensity,this.bias=e.bias,this.radius=e.radius,this.autoUpdate=e.autoUpdate,this.needsUpdate=e.needsUpdate,this.normalBias=e.normalBias,this.blurSamples=e.blurSamples,this.mapSize.copy(e.mapSize),this.biasNode=e.biasNode,this}clone(){return new this.constructor().copy(this)}toJSON(){let e={};return e.intensity=this.intensity,e.bias=this.bias,e.normalBias=this.normalBias,e.radius=this.radius,e.blurSamples=this.blurSamples,e.mapSize=this.mapSize.toArray(),e.camera=this.camera.toJSON(!1).object,delete e.camera.matrix,e}},Ko=new G,Jo=new an,ci=new G,Js=class extends vn{constructor(){super(),this.isCamera=!0,this.type="Camera",this.matrixWorldInverse=new wt,this.projectionMatrix=new wt,this.projectionMatrixInverse=new wt,this.coordinateSystem=ei,this._reversedDepth=!1}get reversedDepth(){return this._reversedDepth}copy(e,t){return super.copy(e,t),this.matrixWorldInverse.copy(e.matrixWorldInverse),this.projectionMatrix.copy(e.projectionMatrix),this.projectionMatrixInverse.copy(e.projectionMatrixInverse),this.coordinateSystem=e.coordinateSystem,this}getWorldDirection(e){return super.getWorldDirection(e).negate()}updateMatrixWorld(e){super.updateMatrixWorld(e),this.matrixWorld.decompose(Ko,Jo,ci),ci.x===1&&ci.y===1&&ci.z===1?this.matrixWorldInverse.copy(this.matrixWorld).invert():this.matrixWorldInverse.compose(Ko,Jo,ci.set(1,1,1)).invert()}updateWorldMatrix(e,t,i=!1){super.updateWorldMatrix(e,t,i),this.matrixWorld.decompose(Ko,Jo,ci),ci.x===1&&ci.y===1&&ci.z===1?this.matrixWorldInverse.copy(this.matrixWorld).invert():this.matrixWorldInverse.compose(Ko,Jo,ci.set(1,1,1)).invert()}clone(){return new this.constructor().copy(this)}},Oi=new G,Th=new We,Ah=new We,gn=class extends Js{constructor(e=50,t=1,i=.1,r=2e3){super(),this.isPerspectiveCamera=!0,this.type="PerspectiveCamera",this.fov=e,this.zoom=1,this.near=i,this.far=r,this.focus=10,this.aspect=t,this.view=null,this.filmGauge=35,this.filmOffset=0,this.updateProjectionMatrix()}copy(e,t){return super.copy(e,t),this.fov=e.fov,this.zoom=e.zoom,this.near=e.near,this.far=e.far,this.focus=e.focus,this.aspect=e.aspect,this.view=e.view===null?null:Object.assign({},e.view),this.filmGauge=e.filmGauge,this.filmOffset=e.filmOffset,this}setFocalLength(e){let t=.5*this.getFilmHeight()/e;this.fov=Yr*2*Math.atan(t),this.updateProjectionMatrix()}getFocalLength(){let e=Math.tan(As*.5*this.fov);return .5*this.getFilmHeight()/e}getEffectiveFOV(){return Yr*2*Math.atan(Math.tan(As*.5*this.fov)/this.zoom)}getFilmWidth(){return this.filmGauge*Math.min(this.aspect,1)}getFilmHeight(){return this.filmGauge/Math.max(this.aspect,1)}getViewBounds(e,t,i){Oi.set(-1,-1,.5).applyMatrix4(this.projectionMatrixInverse),t.set(Oi.x,Oi.y).multiplyScalar(-e/Oi.z),Oi.set(1,1,.5).applyMatrix4(this.projectionMatrixInverse),i.set(Oi.x,Oi.y).multiplyScalar(-e/Oi.z)}getViewSize(e,t){return this.getViewBounds(e,Th,Ah),t.subVectors(Ah,Th)}setViewOffset(e,t,i,r,s,o){this.aspect=e/t,this.view===null&&(this.view={enabled:!0,fullWidth:1,fullHeight:1,offsetX:0,offsetY:0,width:1,height:1}),this.view.enabled=!0,this.view.fullWidth=e,this.view.fullHeight=t,this.view.offsetX=i,this.view.offsetY=r,this.view.width=s,this.view.height=o,this.updateProjectionMatrix()}clearViewOffset(){this.view!==null&&(this.view.enabled=!1),this.updateProjectionMatrix()}updateProjectionMatrix(){let e=this.near,t=e*Math.tan(As*.5*this.fov)/this.zoom,i=2*t,r=this.aspect*i,s=-.5*r,o=this.view;if(this.view!==null&&this.view.enabled){let l=o.fullWidth,c=o.fullHeight;s+=o.offsetX*r/l,t-=o.offsetY*i/c,r*=o.width/l,i*=o.height/c}let a=this.filmOffset;a!==0&&(s+=e*a/this.getFilmWidth()),this.projectionMatrix.makePerspective(s,s+r,t,t-i,e,this.far,this.coordinateSystem,this.reversedDepth),this.projectionMatrixInverse.copy(this.projectionMatrix).invert()}toJSON(e){let t=super.toJSON(e);return t.object.fov=this.fov,t.object.zoom=this.zoom,t.object.near=this.near,t.object.far=this.far,t.object.focus=this.focus,t.object.aspect=this.aspect,this.view!==null&&(t.object.view=Object.assign({},this.view)),t.object.filmGauge=this.filmGauge,t.object.filmOffset=this.filmOffset,t}};var Mc=class extends Ks{constructor(){super(new gn(90,1,.5,500)),this.isPointLightShadow=!0}},js=class extends ts{constructor(e,t,i=0,r=2){super(e,t),this.isPointLight=!0,this.type="PointLight",this.distance=i,this.decay=r,this.shadow=new Mc}get power(){return this.intensity*4*Math.PI}set power(e){this.intensity=e/(4*Math.PI)}dispose(){super.dispose(),this.shadow.dispose()}copy(e,t){return super.copy(e,t),this.distance=e.distance,this.decay=e.decay,this.shadow=e.shadow.clone(),this}toJSON(e){let t=super.toJSON(e);return t.object.distance=this.distance,t.object.decay=this.decay,t.object.shadow=this.shadow.toJSON(),t}},ns=class extends Js{constructor(e=-1,t=1,i=1,r=-1,s=.1,o=2e3){super(),this.isOrthographicCamera=!0,this.type="OrthographicCamera",this.zoom=1,this.view=null,this.left=e,this.right=t,this.top=i,this.bottom=r,this.near=s,this.far=o,this.updateProjectionMatrix()}copy(e,t){return super.copy(e,t),this.left=e.left,this.right=e.right,this.top=e.top,this.bottom=e.bottom,this.near=e.near,this.far=e.far,this.zoom=e.zoom,this.view=e.view===null?null:Object.assign({},e.view),this}setViewOffset(e,t,i,r,s,o){this.view===null&&(this.view={enabled:!0,fullWidth:1,fullHeight:1,offsetX:0,offsetY:0,width:1,height:1}),this.view.enabled=!0,this.view.fullWidth=e,this.view.fullHeight=t,this.view.offsetX=i,this.view.offsetY=r,this.view.width=s,this.view.height=o,this.updateProjectionMatrix()}clearViewOffset(){this.view!==null&&(this.view.enabled=!1),this.updateProjectionMatrix()}updateProjectionMatrix(){let e=(this.right-this.left)/(2*this.zoom),t=(this.top-this.bottom)/(2*this.zoom),i=(this.right+this.left)/2,r=(this.top+this.bottom)/2,s=i-e,o=i+e,a=r+t,l=r-t;if(this.view!==null&&this.view.enabled){let c=(this.right-this.left)/this.view.fullWidth/this.zoom,d=(this.top-this.bottom)/this.view.fullHeight/this.zoom;s+=c*this.view.offsetX,o=s+c*this.view.width,a-=d*this.view.offsetY,l=a-d*this.view.height}this.projectionMatrix.makeOrthographic(s,o,a,l,this.near,this.far,this.coordinateSystem,this.reversedDepth),this.projectionMatrixInverse.copy(this.projectionMatrix).invert()}toJSON(e){let t=super.toJSON(e);return t.object.zoom=this.zoom,t.object.left=this.left,t.object.right=this.right,t.object.top=this.top,t.object.bottom=this.bottom,t.object.near=this.near,t.object.far=this.far,this.view!==null&&(t.object.view=Object.assign({},this.view)),t}},Sc=class extends Ks{constructor(){super(new ns(-5,5,5,-5,.5,500)),this.isDirectionalLightShadow=!0}},Qs=class extends ts{constructor(e,t){super(e,t),this.isDirectionalLight=!0,this.type="DirectionalLight",this.position.copy(vn.DEFAULT_UP),this.updateMatrix(),this.target=new vn,this.shadow=new Sc}dispose(){super.dispose(),this.shadow.dispose()}copy(e){return super.copy(e),this.target=e.target.clone(),this.shadow=e.shadow.clone(),this}toJSON(e){let t=super.toJSON(e);return t.object.shadow=this.shadow.toJSON(),t.object.target=this.target.uuid,t}};var Hr=-90,Gr=1,Ta=class extends vn{constructor(e,t,i){super(),this.type="CubeCamera",this.renderTarget=i,this.coordinateSystem=null,this.activeMipmapLevel=0;let r=new gn(Hr,Gr,e,t);r.layers=this.layers,this.add(r);let s=new gn(Hr,Gr,e,t);s.layers=this.layers,this.add(s);let o=new gn(Hr,Gr,e,t);o.layers=this.layers,this.add(o);let a=new gn(Hr,Gr,e,t);a.layers=this.layers,this.add(a);let l=new gn(Hr,Gr,e,t);l.layers=this.layers,this.add(l);let c=new gn(Hr,Gr,e,t);c.layers=this.layers,this.add(c)}updateCoordinateSystem(){let e=this.coordinateSystem,t=this.children.concat(),[i,r,s,o,a,l]=t;for(let c of t)this.remove(c);if(e===ei)i.up.set(0,1,0),i.lookAt(1,0,0),r.up.set(0,1,0),r.lookAt(-1,0,0),s.up.set(0,0,-1),s.lookAt(0,1,0),o.up.set(0,0,1),o.lookAt(0,-1,0),a.up.set(0,1,0),a.lookAt(0,0,1),l.up.set(0,1,0),l.lookAt(0,0,-1);else if(e===qr)i.up.set(0,-1,0),i.lookAt(-1,0,0),r.up.set(0,-1,0),r.lookAt(1,0,0),s.up.set(0,0,1),s.lookAt(0,1,0),o.up.set(0,0,-1),o.lookAt(0,-1,0),a.up.set(0,-1,0),a.lookAt(0,0,1),l.up.set(0,-1,0),l.lookAt(0,0,-1);else throw new Error("THREE.CubeCamera.updateCoordinateSystem(): Invalid coordinate system: "+e);for(let c of t)this.add(c),c.updateMatrixWorld()}update(e,t){this.parent===null&&this.updateMatrixWorld();let{renderTarget:i,activeMipmapLevel:r}=this;this.coordinateSystem!==e.coordinateSystem&&(this.coordinateSystem=e.coordinateSystem,this.updateCoordinateSystem());let[s,o,a,l,c,d]=this.children,m=e.getRenderTarget(),u=e.getActiveCubeFace(),g=e.getActiveMipmapLevel(),_=e.xr.enabled;e.xr.enabled=!1;let v=i.texture.generateMipmaps;i.texture.generateMipmaps=!1;let f=!1;e.isWebGLRenderer===!0?f=e.state.buffers.depth.getReversed():f=e.reversedDepthBuffer,e.setRenderTarget(i,0,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,s),e.setRenderTarget(i,1,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,o),e.setRenderTarget(i,2,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,a),e.setRenderTarget(i,3,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,l),e.setRenderTarget(i,4,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,c),i.texture.generateMipmaps=v,e.setRenderTarget(i,5,r),f&&e.autoClear===!1&&e.clearDepth(),e.render(t,d),e.setRenderTarget(m,u,g),e.xr.enabled=_,i.texture.needsPMREMUpdate=!0}},Aa=class extends gn{constructor(e=[]){super(),this.isArrayCamera=!0,this.isMultiViewCamera=!1,this.cameras=e}};var jc="\\[\\]\\.:\\/",rm=new RegExp("["+jc+"]","g"),Qc="[^"+jc+"]",sm="[^"+jc.replace("\\.","")+"]",om=/((?:WC+[\/:])*)/.source.replace("WC",Qc),am=/(WCOD+)?/.source.replace("WCOD",sm),lm=/(?:\.(WC+)(?:\[(.+)\])?)?/.source.replace("WC",Qc),cm=/\.(WC+)(?:\[(.+)\])?/.source.replace("WC",Qc),um=new RegExp("^"+om+am+lm+cm+"$"),hm=["material","materials","bones","map"],wc=class{constructor(e,t,i){let r=i||Nt.parseTrackName(t);this._targetGroup=e,this._bindings=e.subscribe_(t,r)}getValue(e,t){this.bind();let i=this._targetGroup.nCachedObjects_,r=this._bindings[i];r!==void 0&&r.getValue(e,t)}setValue(e,t){let i=this._bindings;for(let r=this._targetGroup.nCachedObjects_,s=i.length;r!==s;++r)i[r].setValue(e,t)}bind(){let e=this._bindings;for(let t=this._targetGroup.nCachedObjects_,i=e.length;t!==i;++t)e[t].bind()}unbind(){let e=this._bindings;for(let t=this._targetGroup.nCachedObjects_,i=e.length;t!==i;++t)e[t].unbind()}},Nt=class n{constructor(e,t,i){this.path=t,this.parsedPath=i||n.parseTrackName(t),this.node=n.findNode(e,this.parsedPath.nodeName),this.rootNode=e,this.getValue=this._getValue_unbound,this.setValue=this._setValue_unbound}static create(e,t,i){return e&&e.isAnimationObjectGroup?new n.Composite(e,t,i):new n(e,t,i)}static sanitizeNodeName(e){return e.replace(/\s/g,"_").replace(rm,"")}static parseTrackName(e){let t=um.exec(e);if(t===null)throw new Error("THREE.PropertyBinding: Cannot parse trackName: "+e);let i={nodeName:t[2],objectName:t[3],objectIndex:t[4],propertyName:t[5],propertyIndex:t[6]},r=i.nodeName&&i.nodeName.lastIndexOf(".");if(r!==void 0&&r!==-1){let s=i.nodeName.substring(r+1);hm.indexOf(s)!==-1&&(i.nodeName=i.nodeName.substring(0,r),i.objectName=s)}if(i.propertyName===null||i.propertyName.length===0)throw new Error("THREE.PropertyBinding: can not parse propertyName from trackName: "+e);return i}static findNode(e,t){if(t===void 0||t===""||t==="."||t===-1||t===e.name||t===e.uuid)return e;if(e.skeleton){let i=e.skeleton.getBoneByName(t);if(i!==void 0)return i}if(e.children){let i=function(s){for(let o=0;o<s.length;o++){let a=s[o];if(a.name===t||a.uuid===t)return a;let l=i(a.children);if(l)return l}return null},r=i(e.children);if(r)return r}return null}_getValue_unavailable(){}_setValue_unavailable(){}_getValue_direct(e,t){e[t]=this.targetObject[this.propertyName]}_getValue_array(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)e[t++]=i[r]}_getValue_arrayElement(e,t){e[t]=this.resolvedProperty[this.propertyIndex]}_getValue_toArray(e,t){this.resolvedProperty.toArray(e,t)}_setValue_direct(e,t){this.targetObject[this.propertyName]=e[t]}_setValue_direct_setNeedsUpdate(e,t){this.targetObject[this.propertyName]=e[t],this.targetObject.needsUpdate=!0}_setValue_direct_setMatrixWorldNeedsUpdate(e,t){this.targetObject[this.propertyName]=e[t],this.targetObject.matrixWorldNeedsUpdate=!0}_setValue_array(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)i[r]=e[t++]}_setValue_array_setNeedsUpdate(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)i[r]=e[t++];this.targetObject.needsUpdate=!0}_setValue_array_setMatrixWorldNeedsUpdate(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)i[r]=e[t++];this.targetObject.matrixWorldNeedsUpdate=!0}_setValue_arrayElement(e,t){this.resolvedProperty[this.propertyIndex]=e[t]}_setValue_arrayElement_setNeedsUpdate(e,t){this.resolvedProperty[this.propertyIndex]=e[t],this.targetObject.needsUpdate=!0}_setValue_arrayElement_setMatrixWorldNeedsUpdate(e,t){this.resolvedProperty[this.propertyIndex]=e[t],this.targetObject.matrixWorldNeedsUpdate=!0}_setValue_fromArray(e,t){this.resolvedProperty.fromArray(e,t)}_setValue_fromArray_setNeedsUpdate(e,t){this.resolvedProperty.fromArray(e,t),this.targetObject.needsUpdate=!0}_setValue_fromArray_setMatrixWorldNeedsUpdate(e,t){this.resolvedProperty.fromArray(e,t),this.targetObject.matrixWorldNeedsUpdate=!0}_getValue_unbound(e,t){this.bind(),this.getValue(e,t)}_setValue_unbound(e,t){this.bind(),this.setValue(e,t)}bind(){let e=this.node,t=this.parsedPath,i=t.objectName,r=t.propertyName,s=t.propertyIndex;if(e||(e=n.findNode(this.rootNode,t.nodeName),this.node=e),this.getValue=this._getValue_unavailable,this.setValue=this._setValue_unavailable,!e){$e("PropertyBinding: No target node found for track: "+this.path+".");return}if(i){let c=t.objectIndex;switch(i){case"materials":if(!e.material){Ze("PropertyBinding: Can not bind to material as node does not have a material.",this);return}if(!e.material.materials){Ze("PropertyBinding: Can not bind to material.materials as node.material does not have a materials array.",this);return}e=e.material.materials;break;case"bones":if(!e.skeleton){Ze("PropertyBinding: Can not bind to bones as node does not have a skeleton.",this);return}e=e.skeleton.bones;for(let d=0;d<e.length;d++)if(e[d].name===c){c=d;break}break;case"map":if("map"in e){e=e.map;break}if(!e.material){Ze("PropertyBinding: Can not bind to material as node does not have a material.",this);return}if(!e.material.map){Ze("PropertyBinding: Can not bind to material.map as node.material does not have a map.",this);return}e=e.material.map;break;default:if(e[i]===void 0){Ze("PropertyBinding: Can not bind to objectName of node undefined.",this);return}e=e[i]}if(c!==void 0){if(e[c]===void 0){Ze("PropertyBinding: Trying to bind to objectIndex of objectName, but is undefined.",this,e);return}e=e[c]}}let o=e[r];if(o===void 0){let c=t.nodeName;Ze("PropertyBinding: Trying to update property for track: "+c+"."+r+" but it wasn't found.",e);return}let a=this.Versioning.None;this.targetObject=e,e.isMaterial===!0?a=this.Versioning.NeedsUpdate:e.isObject3D===!0&&(a=this.Versioning.MatrixWorldNeedsUpdate);let l=this.BindingType.Direct;if(s!==void 0){if(r==="morphTargetInfluences"){if(!e.geometry){Ze("PropertyBinding: Can not bind to morphTargetInfluences because node does not have a geometry.",this);return}if(!e.geometry.morphAttributes){Ze("PropertyBinding: Can not bind to morphTargetInfluences because node does not have a geometry.morphAttributes.",this);return}e.morphTargetDictionary[s]!==void 0&&(s=e.morphTargetDictionary[s])}l=this.BindingType.ArrayElement,this.resolvedProperty=o,this.propertyIndex=s}else o.fromArray!==void 0&&o.toArray!==void 0?(l=this.BindingType.HasFromToArray,this.resolvedProperty=o):Array.isArray(o)?(l=this.BindingType.EntireArray,this.resolvedProperty=o):this.propertyName=r;this.getValue=this.GetterByBindingType[l],this.setValue=this.SetterByBindingTypeAndVersioning[l][a]}unbind(){this.node=null,this.getValue=this._getValue_unbound,this.setValue=this._setValue_unbound}};Nt.Composite=wc;Nt.prototype.BindingType={Direct:0,EntireArray:1,ArrayElement:2,HasFromToArray:3};Nt.prototype.Versioning={None:0,NeedsUpdate:1,MatrixWorldNeedsUpdate:2};Nt.prototype.GetterByBindingType=[Nt.prototype._getValue_direct,Nt.prototype._getValue_array,Nt.prototype._getValue_arrayElement,Nt.prototype._getValue_toArray];Nt.prototype.SetterByBindingTypeAndVersioning=[[Nt.prototype._setValue_direct,Nt.prototype._setValue_direct_setNeedsUpdate,Nt.prototype._setValue_direct_setMatrixWorldNeedsUpdate],[Nt.prototype._setValue_array,Nt.prototype._setValue_array_setNeedsUpdate,Nt.prototype._setValue_array_setMatrixWorldNeedsUpdate],[Nt.prototype._setValue_arrayElement,Nt.prototype._setValue_arrayElement_setNeedsUpdate,Nt.prototype._setValue_arrayElement_setMatrixWorldNeedsUpdate],[Nt.prototype._setValue_fromArray,Nt.prototype._setValue_fromArray_setNeedsUpdate,Nt.prototype._setValue_fromArray_setMatrixWorldNeedsUpdate]];var _b=new Float32Array(1);var Rh=new wt,eo=class{constructor(e,t,i=0,r=1/0){this.ray=new Hi(e,t),this.near=i,this.far=r,this.camera=null,this.layers=new Kr,this.params={Mesh:{},Line:{threshold:1},LOD:{},Points:{threshold:1},Sprite:{}}}set(e,t){this.ray.set(e,t)}setFromCamera(e,t){t.isPerspectiveCamera?(this.ray.origin.setFromMatrixPosition(t.matrixWorld),this.ray.direction.set(e.x,e.y,.5).unproject(t).sub(this.ray.origin).normalize(),this.camera=t):t.isOrthographicCamera?(this.ray.origin.set(e.x,e.y,t.projectionMatrix.elements[14]).unproject(t),this.ray.direction.set(0,0,-1).transformDirection(t.matrixWorld),this.camera=t):Ze("Raycaster: Unsupported camera type: "+t.type)}setFromXRController(e){return Rh.identity().extractRotation(e.matrixWorld),this.ray.origin.setFromMatrixPosition(e.matrixWorld),this.ray.direction.set(0,0,-1).applyMatrix4(Rh),this}intersectObject(e,t=!0,i=[]){return Ec(e,this,i,t),i.sort(Ch),i}intersectObjects(e,t=!0,i=[]){for(let r=0,s=e.length;r<s;r++)Ec(e[r],this,i,t);return i.sort(Ch),i}};function Ch(n,e){return n.distance-e.distance}function Ec(n,e,t,i){let r=!0;if(n.layers.test(e.layers)&&n.raycast(e,t)===!1&&(r=!1),r===!0&&i===!0){let s=n.children;for(let o=0,a=s.length;o<a;o++)Ec(s[o],e,t,!0)}}var is=class{constructor(e=1,t=0,i=0){this.radius=e,this.phi=t,this.theta=i}set(e,t,i){return this.radius=e,this.phi=t,this.theta=i,this}copy(e){return this.radius=e.radius,this.phi=e.phi,this.theta=e.theta,this}makeSafe(){return this.phi=at(this.phi,1e-6,Math.PI-1e-6),this}setFromVector3(e){return this.setFromCartesianCoords(e.x,e.y,e.z)}setFromCartesianCoords(e,t,i){return this.radius=Math.sqrt(e*e+t*t+i*i),this.radius===0?(this.theta=0,this.phi=0):(this.theta=Math.atan2(e,i),this.phi=Math.acos(at(t/this.radius,-1,1))),this}clone(){return new this.constructor().copy(this)}};var Tc=class n{static{n.prototype.isMatrix2=!0}constructor(e,t,i,r){this.elements=[1,0,0,1],e!==void 0&&this.set(e,t,i,r)}identity(){return this.set(1,0,0,1),this}fromArray(e,t=0){for(let i=0;i<4;i++)this.elements[i]=e[i+t];return this}set(e,t,i,r){let s=this.elements;return s[0]=e,s[2]=t,s[1]=i,s[3]=r,this}};var to=class extends ti{constructor(e,t=null){super(),this.object=e,this.domElement=t,this.enabled=!0,this.state=-1,this.keys={},this.mouseButtons={LEFT:null,MIDDLE:null,RIGHT:null},this.touches={ONE:null,TWO:null}}connect(e){this.domElement!==null&&this.disconnect(),this.domElement=e}disconnect(){}dispose(){}update(){}};function eu(n,e,t,i){let r=dm(i);switch(t){case Xc:return n*e;case Da:return n*e/r.components*r.byteLength;case Na:return n*e/r.components*r.byteLength;case er:return n*e*2/r.components*r.byteLength;case Ua:return n*e*2/r.components*r.byteLength;case qc:return n*e*3/r.components*r.byteLength;case Fn:return n*e*4/r.components*r.byteLength;case Fa:return n*e*4/r.components*r.byteLength;case ro:case so:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*8;case oo:case ao:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*16;case Ba:case Ha:return Math.max(n,16)*Math.max(e,8)/4;case Oa:case za:return Math.max(n,8)*Math.max(e,8)/2;case Ga:case Va:case Xa:case qa:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*8;case Wa:case lo:case $a:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*16;case Ya:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*16;case Za:return Math.floor((n+4)/5)*Math.floor((e+3)/4)*16;case Ka:return Math.floor((n+4)/5)*Math.floor((e+4)/5)*16;case Ja:return Math.floor((n+5)/6)*Math.floor((e+4)/5)*16;case ja:return Math.floor((n+5)/6)*Math.floor((e+5)/6)*16;case Qa:return Math.floor((n+7)/8)*Math.floor((e+4)/5)*16;case el:return Math.floor((n+7)/8)*Math.floor((e+5)/6)*16;case tl:return Math.floor((n+7)/8)*Math.floor((e+7)/8)*16;case nl:return Math.floor((n+9)/10)*Math.floor((e+4)/5)*16;case il:return Math.floor((n+9)/10)*Math.floor((e+5)/6)*16;case rl:return Math.floor((n+9)/10)*Math.floor((e+7)/8)*16;case sl:return Math.floor((n+9)/10)*Math.floor((e+9)/10)*16;case ol:return Math.floor((n+11)/12)*Math.floor((e+9)/10)*16;case al:return Math.floor((n+11)/12)*Math.floor((e+11)/12)*16;case ll:case cl:case ul:return Math.ceil(n/4)*Math.ceil(e/4)*16;case hl:case dl:return Math.ceil(n/4)*Math.ceil(e/4)*8;case co:case fl:return Math.ceil(n/4)*Math.ceil(e/4)*16}throw new Error(`Unable to determine texture byte length for ${t} format.`)}function dm(n){switch(n){case Un:case Hc:return{byteLength:1,components:1};case as:case Gc:case ri:return{byteLength:2,components:1};case La:case ka:return{byteLength:2,components:4};case ii:case Ia:case $n:return{byteLength:4,components:1};case Vc:case Wc:return{byteLength:4,components:3}}throw new Error(`THREE.TextureUtils: Unknown texture type ${n}.`)}typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("register",{detail:{revision:"186"}}));typeof window<"u"&&(window.__THREE__?$e("WARNING: Multiple instances of Three.js being imported."):window.__THREE__="186");function Od(){let n=null,e=!1,t=null,i=null;function r(s,o){i=n.requestAnimationFrame(r),t(s,o)}return{start:function(){e!==!0&&t!==null&&n!==null&&(i=n.requestAnimationFrame(r),e=!0)},stop:function(){n!==null&&n.cancelAnimationFrame(i),e=!1},setAnimationLoop:function(s){t=s},setContext:function(s){n=s}}}function pm(n){let e=new WeakMap;function t(a,l){let c=a.array,d=a.usage,m=c.byteLength,u=n.createBuffer();n.bindBuffer(l,u),n.bufferData(l,c,d),a.onUploadCallback();let g;if(c instanceof Float32Array)g=n.FLOAT;else if(typeof Float16Array<"u"&&c instanceof Float16Array)g=n.HALF_FLOAT;else if(c instanceof Uint16Array)a.isFloat16BufferAttribute?g=n.HALF_FLOAT:g=n.UNSIGNED_SHORT;else if(c instanceof Int16Array)g=n.SHORT;else if(c instanceof Uint32Array)g=n.UNSIGNED_INT;else if(c instanceof Int32Array)g=n.INT;else if(c instanceof Int8Array)g=n.BYTE;else if(c instanceof Uint8Array)g=n.UNSIGNED_BYTE;else if(c instanceof Uint8ClampedArray)g=n.UNSIGNED_BYTE;else throw new Error("THREE.WebGLAttributes: Unsupported buffer data format: "+c);return{buffer:u,type:g,bytesPerElement:c.BYTES_PER_ELEMENT,version:a.version,size:m}}function i(a,l,c){let d=l.array,m=l.updateRanges;if(n.bindBuffer(c,a),m.length===0)n.bufferSubData(c,0,d);else{m.sort((g,_)=>g.start-_.start);let u=0;for(let g=1;g<m.length;g++){let _=m[u],v=m[g];v.start<=_.start+_.count+1?_.count=Math.max(_.count,v.start+v.count-_.start):(++u,m[u]=v)}m.length=u+1;for(let g=0,_=m.length;g<_;g++){let v=m[g];n.bufferSubData(c,v.start*d.BYTES_PER_ELEMENT,d,v.start,v.count)}l.clearUpdateRanges()}l.onUploadCallback()}function r(a){return a.isInterleavedBufferAttribute&&(a=a.data),e.get(a)}function s(a){a.isInterleavedBufferAttribute&&(a=a.data);let l=e.get(a);l&&(n.deleteBuffer(l.buffer),e.delete(a))}function o(a,l){if(a.isInterleavedBufferAttribute&&(a=a.data),a.isGLBufferAttribute){let d=e.get(a);(!d||d.version<a.version)&&e.set(a,{buffer:a.buffer,type:a.type,bytesPerElement:a.elementSize,version:a.version});return}let c=e.get(a);if(c===void 0)e.set(a,t(a,l));else if(c.version<a.version){if(c.size!==a.array.byteLength)throw new Error("THREE.WebGLAttributes: The size of the buffer attribute's array buffer does not match the original size. Resizing buffer attributes is not supported.");i(c.buffer,a,l),c.version=a.version}}return{get:r,remove:s,update:o}}var mm=`#ifdef USE_ALPHAHASH
	if ( diffuseColor.a < getAlphaHashThreshold( vPosition ) ) discard;
#endif`,gm=`#ifdef USE_ALPHAHASH
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
#endif`,xm=`#ifdef USE_ALPHAMAP
	diffuseColor.a *= texture2D( alphaMap, vAlphaMapUv ).g;
#endif`,vm=`#ifdef USE_ALPHAMAP
	uniform sampler2D alphaMap;
#endif`,_m=`#ifdef USE_ALPHATEST
	#ifdef ALPHA_TO_COVERAGE
	diffuseColor.a = smoothstep( alphaTest, alphaTest + fwidth( diffuseColor.a ), diffuseColor.a );
	if ( diffuseColor.a == 0.0 ) discard;
	#else
	if ( diffuseColor.a < alphaTest ) discard;
	#endif
#endif`,ym=`#ifdef USE_ALPHATEST
	uniform float alphaTest;
#endif`,bm=`#ifdef USE_AOMAP
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
#endif`,Mm=`#ifdef USE_AOMAP
	uniform sampler2D aoMap;
	uniform float aoMapIntensity;
#endif`,Sm=`#ifdef USE_BATCHING
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
#endif`,wm=`#ifdef USE_BATCHING
	mat4 batchingMatrix = getBatchingMatrix( getIndirectIndex( gl_DrawID ) );
#endif`,Em=`vec3 transformed = vec3( position );
#ifdef USE_ALPHAHASH
	vPosition = vec3( position );
#endif`,Tm=`vec3 objectNormal = vec3( normal );
#ifdef USE_TANGENT
	vec3 objectTangent = vec3( tangent.xyz );
#endif`,Am=`float G_BlinnPhong_Implicit( ) {
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
} // validated`,Rm=`#ifdef USE_IRIDESCENCE
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
#endif`,Cm=`#ifdef USE_BUMPMAP
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
#endif`,Pm=`#if NUM_CLIPPING_PLANES > 0
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
#endif`,Im=`#if NUM_CLIPPING_PLANES > 0
	varying vec3 vClipPosition;
	uniform vec4 clippingPlanes[ NUM_CLIPPING_PLANES ];
#endif`,Lm=`#if NUM_CLIPPING_PLANES > 0
	varying vec3 vClipPosition;
#endif`,km=`#if NUM_CLIPPING_PLANES > 0
	vClipPosition = - mvPosition.xyz;
#endif`,Dm=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA )
	diffuseColor *= vColor;
#endif`,Nm=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA )
	varying vec4 vColor;
#endif`,Um=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA ) || defined( USE_INSTANCING_COLOR ) || defined( USE_BATCHING_COLOR )
	varying vec4 vColor;
#endif`,Fm=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA ) || defined( USE_INSTANCING_COLOR ) || defined( USE_BATCHING_COLOR )
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
#endif`,Om=`#define PI 3.141592653589793
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
} // validated`,Bm=`#ifdef ENVMAP_TYPE_CUBE_UV
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
#endif`,zm=`vec3 transformedNormal = objectNormal;
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
#endif`,Hm=`#ifdef USE_DISPLACEMENTMAP
	uniform sampler2D displacementMap;
	uniform float displacementScale;
	uniform float displacementBias;
#endif`,Gm=`#ifdef USE_DISPLACEMENTMAP
	transformed += normalize( objectNormal ) * ( texture2D( displacementMap, vDisplacementMapUv ).x * displacementScale + displacementBias );
#endif`,Vm=`#ifdef USE_EMISSIVEMAP
	vec4 emissiveColor = texture2D( emissiveMap, vEmissiveMapUv );
	#ifdef DECODE_VIDEO_TEXTURE_EMISSIVE
		emissiveColor = sRGBTransferEOTF( emissiveColor );
	#endif
	totalEmissiveRadiance *= emissiveColor.rgb;
#endif`,Wm=`#ifdef USE_EMISSIVEMAP
	uniform sampler2D emissiveMap;
#endif`,Xm="gl_FragColor = linearToOutputTexel( gl_FragColor );",qm=`vec4 LinearTransferOETF( in vec4 value ) {
	return value;
}
vec4 sRGBTransferEOTF( in vec4 value ) {
	return vec4( mix( pow( value.rgb * 0.9478672986 + vec3( 0.0521327014 ), vec3( 2.4 ) ), value.rgb * 0.0773993808, vec3( lessThanEqual( value.rgb, vec3( 0.04045 ) ) ) ), value.a );
}
vec4 sRGBTransferOETF( in vec4 value ) {
	return vec4( mix( pow( value.rgb, vec3( 0.41666 ) ) * 1.055 - vec3( 0.055 ), value.rgb * 12.92, vec3( lessThanEqual( value.rgb, vec3( 0.0031308 ) ) ) ), value.a );
}`,$m=`#ifdef USE_ENVMAP
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
#endif`,Ym=`#ifdef USE_ENVMAP
	uniform float envMapIntensity;
	uniform mat3 envMapRotation;
	#ifdef ENVMAP_TYPE_CUBE
		uniform samplerCube envMap;
	#else
		uniform sampler2D envMap;
	#endif
#endif`,Zm=`#ifdef USE_ENVMAP
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
#endif`,Km=`#ifdef USE_ENVMAP
	#if defined( USE_BUMPMAP ) || defined( USE_NORMALMAP ) || defined( PHONG ) || defined( LAMBERT )
		#define ENV_WORLDPOS
	#endif
	#ifdef ENV_WORLDPOS
		
		varying vec3 vWorldPosition;
	#else
		varying vec3 vReflect;
		uniform float refractionRatio;
	#endif
#endif`,Jm=`#ifdef USE_ENVMAP
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
#endif`,jm=`#ifdef USE_FOG
	vFogDepth = - mvPosition.z;
#endif`,Qm=`#ifdef USE_FOG
	varying float vFogDepth;
#endif`,e0=`#ifdef USE_FOG
	#ifdef FOG_EXP2
		float fogFactor = 1.0 - exp( - fogDensity * fogDensity * vFogDepth * vFogDepth );
	#else
		float fogFactor = smoothstep( fogNear, fogFar, vFogDepth );
	#endif
	gl_FragColor.rgb = mix( gl_FragColor.rgb, fogColor, fogFactor );
#endif`,t0=`#ifdef USE_FOG
	uniform vec3 fogColor;
	varying float vFogDepth;
	#ifdef FOG_EXP2
		uniform float fogDensity;
	#else
		uniform float fogNear;
		uniform float fogFar;
	#endif
#endif`,n0=`#ifdef USE_GRADIENTMAP
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
}`,i0=`#ifdef USE_LIGHTMAP
	uniform sampler2D lightMap;
	uniform float lightMapIntensity;
#endif`,r0=`LambertMaterial material;
material.diffuseColor = diffuseColor.rgb;
material.specularStrength = specularStrength;`,s0=`varying vec3 vViewPosition;
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
#define RE_IndirectDiffuse		RE_IndirectDiffuse_Lambert`,o0=`uniform bool receiveShadow;
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
#include <lightprobes_pars_fragment>`,a0=`#ifdef USE_ENVMAP
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
#endif`,l0=`ToonMaterial material;
material.diffuseColor = diffuseColor.rgb;`,c0=`varying vec3 vViewPosition;
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
#define RE_IndirectDiffuse		RE_IndirectDiffuse_Toon`,u0=`BlinnPhongMaterial material;
material.diffuseColor = diffuseColor.rgb;
material.specularColor = specular;
material.specularShininess = shininess;
material.specularStrength = specularStrength;`,h0=`varying vec3 vViewPosition;
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
#define RE_IndirectDiffuse		RE_IndirectDiffuse_BlinnPhong`,d0=`PhysicalMaterial material;
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
#endif`,f0=`uniform sampler2D dfgLUT;
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
}`,p0=`
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
#endif`,m0=`#if defined( RE_IndirectDiffuse )
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
#endif`,g0=`#if defined( RE_IndirectDiffuse )
	#if defined( LAMBERT ) || defined( PHONG )
		irradiance += iblIrradiance;
	#endif
	RE_IndirectDiffuse( irradiance, geometryPosition, geometryNormal, geometryViewDir, geometryClearcoatNormal, material, reflectedLight );
#endif
#if defined( RE_IndirectSpecular )
	RE_IndirectSpecular( radiance, iblIrradiance, clearcoatRadiance, geometryPosition, geometryNormal, geometryViewDir, geometryClearcoatNormal, material, reflectedLight );
#endif`,x0=`#ifdef USE_LIGHT_PROBES_GRID
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
#endif`,v0=`#if defined( USE_LOGARITHMIC_DEPTH_BUFFER )
	gl_FragDepth = vIsPerspective == 0.0 ? gl_FragCoord.z : log2( vFragDepth ) * logDepthBufFC * 0.5;
#endif`,_0=`#if defined( USE_LOGARITHMIC_DEPTH_BUFFER )
	uniform float logDepthBufFC;
	varying float vFragDepth;
	varying float vIsPerspective;
#endif`,y0=`#ifdef USE_LOGARITHMIC_DEPTH_BUFFER
	varying float vFragDepth;
	varying float vIsPerspective;
#endif`,b0=`#ifdef USE_LOGARITHMIC_DEPTH_BUFFER
	vFragDepth = 1.0 + gl_Position.w;
	vIsPerspective = float( isPerspectiveMatrix( projectionMatrix ) );
#endif`,M0=`#ifdef USE_MAP
	vec4 sampledDiffuseColor = texture2D( map, vMapUv );
	#ifdef DECODE_VIDEO_TEXTURE
		sampledDiffuseColor = sRGBTransferEOTF( sampledDiffuseColor );
	#endif
	diffuseColor *= sampledDiffuseColor;
#endif`,S0=`#ifdef USE_MAP
	uniform sampler2D map;
#endif`,w0=`#if defined( USE_MAP ) || defined( USE_ALPHAMAP )
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
#endif`,E0=`#if defined( USE_POINTS_UV )
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
#endif`,T0=`float metalnessFactor = metalness;
#ifdef USE_METALNESSMAP
	vec4 texelMetalness = texture2D( metalnessMap, vMetalnessMapUv );
	metalnessFactor *= texelMetalness.b;
#endif`,A0=`#ifdef USE_METALNESSMAP
	uniform sampler2D metalnessMap;
#endif`,R0=`#ifdef USE_INSTANCING_MORPH
	float morphTargetInfluences[ MORPHTARGETS_COUNT ];
	float morphTargetBaseInfluence = texelFetch( morphTexture, ivec2( 0, gl_InstanceID ), 0 ).r;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		morphTargetInfluences[i] =  texelFetch( morphTexture, ivec2( i + 1, gl_InstanceID ), 0 ).r;
	}
#endif`,C0=`#if defined( USE_MORPHCOLORS )
	vColor *= morphTargetBaseInfluence;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		#if defined( USE_COLOR_ALPHA )
			if ( morphTargetInfluences[ i ] != 0.0 ) vColor += getMorph( gl_VertexID, i, 2 ) * morphTargetInfluences[ i ];
		#elif defined( USE_COLOR )
			if ( morphTargetInfluences[ i ] != 0.0 ) vColor += getMorph( gl_VertexID, i, 2 ).rgb * morphTargetInfluences[ i ];
		#endif
	}
#endif`,P0=`#ifdef USE_MORPHNORMALS
	objectNormal *= morphTargetBaseInfluence;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		if ( morphTargetInfluences[ i ] != 0.0 ) objectNormal += getMorph( gl_VertexID, i, 1 ).xyz * morphTargetInfluences[ i ];
	}
#endif`,I0=`#ifdef USE_MORPHTARGETS
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
#endif`,L0=`#ifdef USE_MORPHTARGETS
	transformed *= morphTargetBaseInfluence;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		if ( morphTargetInfluences[ i ] != 0.0 ) transformed += getMorph( gl_VertexID, i, 0 ).xyz * morphTargetInfluences[ i ];
	}
#endif`,k0=`float faceDirection = gl_FrontFacing ? 1.0 : - 1.0;
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
vec3 nonPerturbedNormal = normal;`,D0=`#ifdef USE_NORMALMAP_OBJECTSPACE
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
#endif`,N0=`#ifndef FLAT_SHADED
	varying vec3 vNormal;
	#ifdef USE_TANGENT
		varying vec3 vTangent;
		varying vec3 vBitangent;
	#endif
#endif`,U0=`#ifndef FLAT_SHADED
	varying vec3 vNormal;
	#ifdef USE_TANGENT
		varying vec3 vTangent;
		varying vec3 vBitangent;
	#endif
#endif`,F0=`#ifndef FLAT_SHADED
	vNormal = normalize( transformedNormal );
	#ifdef USE_TANGENT
		vTangent = normalize( transformedTangent );
		vBitangent = normalize( cross( vNormal, vTangent ) * tangent.w );
		#ifdef FLIP_SIDED
			vBitangent = - vBitangent;
		#endif
	#endif
#endif`,O0=`#ifdef USE_NORMALMAP
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
#endif`,B0=`#ifdef USE_CLEARCOAT
	vec3 clearcoatNormal = nonPerturbedNormal;
#endif`,z0=`#ifdef USE_CLEARCOAT_NORMALMAP
	vec3 clearcoatMapN = texture2D( clearcoatNormalMap, vClearcoatNormalMapUv ).xyz * 2.0 - 1.0;
	clearcoatMapN.xy *= clearcoatNormalScale;
	clearcoatNormal = normalize( tbn2 * clearcoatMapN );
#endif`,H0=`#ifdef USE_CLEARCOATMAP
	uniform sampler2D clearcoatMap;
#endif
#ifdef USE_CLEARCOAT_NORMALMAP
	uniform sampler2D clearcoatNormalMap;
	uniform vec2 clearcoatNormalScale;
#endif
#ifdef USE_CLEARCOAT_ROUGHNESSMAP
	uniform sampler2D clearcoatRoughnessMap;
#endif`,G0=`#ifdef USE_IRIDESCENCEMAP
	uniform sampler2D iridescenceMap;
#endif
#ifdef USE_IRIDESCENCE_THICKNESSMAP
	uniform sampler2D iridescenceThicknessMap;
#endif`,V0=`#ifdef OPAQUE
diffuseColor.a = 1.0;
#endif
#ifdef USE_TRANSMISSION
diffuseColor.a *= material.transmissionAlpha;
#endif
gl_FragColor = vec4( outgoingLight, diffuseColor.a );`,W0=`vec3 packNormalToRGB( const in vec3 normal ) {
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
}`,X0=`#ifdef PREMULTIPLIED_ALPHA
	gl_FragColor.rgb *= gl_FragColor.a;
#endif`,q0=`vec4 mvPosition = vec4( transformed, 1.0 );
#ifdef USE_BATCHING
	mvPosition = batchingMatrix * mvPosition;
#endif
#ifdef USE_INSTANCING
	mvPosition = instanceMatrix * mvPosition;
#endif
mvPosition = modelViewMatrix * mvPosition;
gl_Position = projectionMatrix * mvPosition;`,$0=`#ifdef DITHERING
	gl_FragColor.rgb = dithering( gl_FragColor.rgb );
#endif`,Y0=`#ifdef DITHERING
	vec3 dithering( vec3 color ) {
		float grid_position = rand( gl_FragCoord.xy );
		vec3 dither_shift_RGB = vec3( 0.25 / 255.0, -0.25 / 255.0, 0.25 / 255.0 );
		dither_shift_RGB = mix( 2.0 * dither_shift_RGB, -2.0 * dither_shift_RGB, grid_position );
		return color + dither_shift_RGB;
	}
#endif`,Z0=`float roughnessFactor = roughness;
#ifdef USE_ROUGHNESSMAP
	vec4 texelRoughness = texture2D( roughnessMap, vRoughnessMapUv );
	roughnessFactor *= texelRoughness.g;
#endif`,K0=`#ifdef USE_ROUGHNESSMAP
	uniform sampler2D roughnessMap;
#endif`,J0=`#if NUM_SPOT_LIGHT_COORDS > 0
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
#endif`,j0=`#if NUM_SPOT_LIGHT_COORDS > 0
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
#endif`,Q0=`#if ( defined( USE_SHADOWMAP ) && ( NUM_DIR_LIGHT_SHADOWS > 0 || NUM_SUN_LIGHT_SHADOWS > 0 || NUM_POINT_LIGHT_SHADOWS > 0 ) ) || ( NUM_SPOT_LIGHT_COORDS > 0 )
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
#endif`,eg=`float getShadowMask() {
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
}`,tg=`#ifdef USE_SKINNING
	mat4 boneMatX = getBoneMatrix( skinIndex.x );
	mat4 boneMatY = getBoneMatrix( skinIndex.y );
	mat4 boneMatZ = getBoneMatrix( skinIndex.z );
	mat4 boneMatW = getBoneMatrix( skinIndex.w );
#endif`,ng=`#ifdef USE_SKINNING
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
#endif`,ig=`#ifdef USE_SKINNING
	vec4 skinVertex = bindMatrix * vec4( transformed, 1.0 );
	vec4 skinned = vec4( 0.0 );
	skinned += boneMatX * skinVertex * skinWeight.x;
	skinned += boneMatY * skinVertex * skinWeight.y;
	skinned += boneMatZ * skinVertex * skinWeight.z;
	skinned += boneMatW * skinVertex * skinWeight.w;
	transformed = ( bindMatrixInverse * skinned ).xyz;
#endif`,rg=`#ifdef USE_SKINNING
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
#endif`,sg=`float specularStrength;
#ifdef USE_SPECULARMAP
	vec4 texelSpecular = texture2D( specularMap, vSpecularMapUv );
	specularStrength = texelSpecular.r;
#else
	specularStrength = 1.0;
#endif`,og=`#ifdef USE_SPECULARMAP
	uniform sampler2D specularMap;
#endif`,ag=`#if defined( TONE_MAPPING )
	gl_FragColor.rgb = toneMapping( gl_FragColor.rgb );
#endif`,lg=`#ifndef saturate
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
vec3 CustomToneMapping( vec3 color ) { return color; }`,cg=`#ifdef USE_TRANSMISSION
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
#endif`,ug=`#ifdef USE_TRANSMISSION
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
#endif`,hg=`#if defined( USE_UV ) || defined( USE_ANISOTROPY )
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
#endif`,dg=`#if defined( USE_UV ) || defined( USE_ANISOTROPY )
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
#endif`,fg=`#if defined( USE_UV ) || defined( USE_ANISOTROPY )
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
#endif`,pg=`#if defined( USE_ENVMAP ) || defined( DISTANCE ) || defined ( USE_SHADOWMAP ) || defined ( USE_TRANSMISSION ) || NUM_SPOT_LIGHT_COORDS > 0
	vec4 worldPosition = vec4( transformed, 1.0 );
	#ifdef USE_BATCHING
		worldPosition = batchingMatrix * worldPosition;
	#endif
	#ifdef USE_INSTANCING
		worldPosition = instanceMatrix * worldPosition;
	#endif
	worldPosition = modelMatrix * worldPosition;
#endif`,mg=`varying vec2 vUv;
uniform mat3 uvTransform;
void main() {
	vUv = ( uvTransform * vec3( uv, 1 ) ).xy;
	gl_Position = vec4( position.xy, 1.0, 1.0 );
}`,gg=`uniform sampler2D t2D;
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
}`,xg=`varying vec3 vWorldDirection;
#include <common>
void main() {
	vWorldDirection = transformDirection( position, modelMatrix );
	#include <begin_vertex>
	#include <project_vertex>
	gl_Position.z = gl_Position.w;
}`,vg=`#ifdef ENVMAP_TYPE_CUBE
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
}`,_g=`varying vec3 vWorldDirection;
#include <common>
void main() {
	vWorldDirection = transformDirection( position, modelMatrix );
	#include <begin_vertex>
	#include <project_vertex>
	gl_Position.z = gl_Position.w;
}`,yg=`uniform samplerCube tCube;
uniform float tFlip;
uniform float opacity;
varying vec3 vWorldDirection;
void main() {
	vec4 texColor = textureCube( tCube, vec3( tFlip * vWorldDirection.x, vWorldDirection.yz ) );
	gl_FragColor = texColor;
	gl_FragColor.a *= opacity;
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
}`,bg=`#include <common>
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
}`,Mg=`#if DEPTH_PACKING == 3200
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
}`,Sg=`#define DISTANCE
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
}`,wg=`#define DISTANCE
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
}`,Eg=`varying vec3 vWorldDirection;
#include <common>
void main() {
	vWorldDirection = transformDirection( position, modelMatrix );
	#include <begin_vertex>
	#include <project_vertex>
}`,Tg=`uniform sampler2D tEquirect;
varying vec3 vWorldDirection;
#include <common>
void main() {
	vec3 direction = normalize( vWorldDirection );
	vec2 sampleUV = equirectUv( direction );
	gl_FragColor = texture2D( tEquirect, sampleUV );
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
}`,Ag=`uniform float scale;
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
}`,Rg=`uniform vec3 diffuse;
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
}`,Cg=`#include <common>
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
}`,Pg=`uniform vec3 diffuse;
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
}`,Ig=`#define LAMBERT
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
}`,Lg=`#define LAMBERT
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
}`,kg=`#define MATCAP
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
}`,Dg=`#define MATCAP
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
}`,Ng=`#define NORMAL
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
}`,Ug=`#define NORMAL
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
}`,Fg=`#define PHONG
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
}`,Og=`#define PHONG
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
}`,Bg=`#define STANDARD
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
}`,zg=`#define STANDARD
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
}`,Hg=`#define TOON
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
}`,Gg=`#define TOON
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
}`,Vg=`uniform float size;
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
}`,Wg=`uniform vec3 diffuse;
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
}`,Xg=`#include <common>
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
}`,qg=`uniform vec3 color;
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
}`,$g=`uniform float rotation;
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
}`,Yg=`uniform vec3 diffuse;
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
}`,it={alphahash_fragment:mm,alphahash_pars_fragment:gm,alphamap_fragment:xm,alphamap_pars_fragment:vm,alphatest_fragment:_m,alphatest_pars_fragment:ym,aomap_fragment:bm,aomap_pars_fragment:Mm,batching_pars_vertex:Sm,batching_vertex:wm,begin_vertex:Em,beginnormal_vertex:Tm,bsdfs:Am,iridescence_fragment:Rm,bumpmap_pars_fragment:Cm,clipping_planes_fragment:Pm,clipping_planes_pars_fragment:Im,clipping_planes_pars_vertex:Lm,clipping_planes_vertex:km,color_fragment:Dm,color_pars_fragment:Nm,color_pars_vertex:Um,color_vertex:Fm,common:Om,cube_uv_reflection_fragment:Bm,defaultnormal_vertex:zm,displacementmap_pars_vertex:Hm,displacementmap_vertex:Gm,emissivemap_fragment:Vm,emissivemap_pars_fragment:Wm,colorspace_fragment:Xm,colorspace_pars_fragment:qm,envmap_fragment:$m,envmap_common_pars_fragment:Ym,envmap_pars_fragment:Zm,envmap_pars_vertex:Km,envmap_physical_pars_fragment:a0,envmap_vertex:Jm,fog_vertex:jm,fog_pars_vertex:Qm,fog_fragment:e0,fog_pars_fragment:t0,gradientmap_pars_fragment:n0,lightmap_pars_fragment:i0,lights_lambert_fragment:r0,lights_lambert_pars_fragment:s0,lights_pars_begin:o0,lights_toon_fragment:l0,lights_toon_pars_fragment:c0,lights_phong_fragment:u0,lights_phong_pars_fragment:h0,lights_physical_fragment:d0,lights_physical_pars_fragment:f0,lights_fragment_begin:p0,lights_fragment_maps:m0,lights_fragment_end:g0,lightprobes_pars_fragment:x0,logdepthbuf_fragment:v0,logdepthbuf_pars_fragment:_0,logdepthbuf_pars_vertex:y0,logdepthbuf_vertex:b0,map_fragment:M0,map_pars_fragment:S0,map_particle_fragment:w0,map_particle_pars_fragment:E0,metalnessmap_fragment:T0,metalnessmap_pars_fragment:A0,morphinstance_vertex:R0,morphcolor_vertex:C0,morphnormal_vertex:P0,morphtarget_pars_vertex:I0,morphtarget_vertex:L0,normal_fragment_begin:k0,normal_fragment_maps:D0,normal_pars_fragment:N0,normal_pars_vertex:U0,normal_vertex:F0,normalmap_pars_fragment:O0,clearcoat_normal_fragment_begin:B0,clearcoat_normal_fragment_maps:z0,clearcoat_pars_fragment:H0,iridescence_pars_fragment:G0,opaque_fragment:V0,packing:W0,premultiplied_alpha_fragment:X0,project_vertex:q0,dithering_fragment:$0,dithering_pars_fragment:Y0,roughnessmap_fragment:Z0,roughnessmap_pars_fragment:K0,shadowmap_pars_fragment:J0,shadowmap_pars_vertex:j0,shadowmap_vertex:Q0,shadowmask_pars_fragment:eg,skinbase_vertex:tg,skinning_pars_vertex:ng,skinning_vertex:ig,skinnormal_vertex:rg,specularmap_fragment:sg,specularmap_pars_fragment:og,tonemapping_fragment:ag,tonemapping_pars_fragment:lg,transmission_fragment:cg,transmission_pars_fragment:ug,uv_pars_fragment:hg,uv_pars_vertex:dg,uv_vertex:fg,worldpos_vertex:pg,background_vert:mg,background_frag:gg,backgroundCube_vert:xg,backgroundCube_frag:vg,cube_vert:_g,cube_frag:yg,depth_vert:bg,depth_frag:Mg,distance_vert:Sg,distance_frag:wg,equirect_vert:Eg,equirect_frag:Tg,linedashed_vert:Ag,linedashed_frag:Rg,meshbasic_vert:Cg,meshbasic_frag:Pg,meshlambert_vert:Ig,meshlambert_frag:Lg,meshmatcap_vert:kg,meshmatcap_frag:Dg,meshnormal_vert:Ng,meshnormal_frag:Ug,meshphong_vert:Fg,meshphong_frag:Og,meshphysical_vert:Bg,meshphysical_frag:zg,meshtoon_vert:Hg,meshtoon_frag:Gg,points_vert:Vg,points_frag:Wg,shadow_vert:Xg,shadow_frag:qg,sprite_vert:$g,sprite_frag:Yg},Te={common:{diffuse:{value:new Ne(16777215)},opacity:{value:1},map:{value:null},mapTransform:{value:new je},alphaMap:{value:null},alphaMapTransform:{value:new je},alphaTest:{value:0}},specularmap:{specularMap:{value:null},specularMapTransform:{value:new je}},envmap:{envMap:{value:null},envMapRotation:{value:new je},reflectivity:{value:1},ior:{value:1.5},refractionRatio:{value:.98},dfgLUT:{value:null}},aomap:{aoMap:{value:null},aoMapIntensity:{value:1},aoMapTransform:{value:new je}},lightmap:{lightMap:{value:null},lightMapIntensity:{value:1},lightMapTransform:{value:new je}},bumpmap:{bumpMap:{value:null},bumpMapTransform:{value:new je},bumpScale:{value:1}},normalmap:{normalMap:{value:null},normalMapTransform:{value:new je},normalScale:{value:new We(1,1)}},displacementmap:{displacementMap:{value:null},displacementMapTransform:{value:new je},displacementScale:{value:1},displacementBias:{value:0}},emissivemap:{emissiveMap:{value:null},emissiveMapTransform:{value:new je}},metalnessmap:{metalnessMap:{value:null},metalnessMapTransform:{value:new je}},roughnessmap:{roughnessMap:{value:null},roughnessMapTransform:{value:new je}},gradientmap:{gradientMap:{value:null}},fog:{fogDensity:{value:25e-5},fogNear:{value:1},fogFar:{value:2e3},fogColor:{value:new Ne(16777215)}},lights:{ambientLightColor:{value:[]},lightProbe:{value:[]},sunLights:{value:[],properties:{direction:{},color:{}}},sunLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{}}},sunShadowMatrix:{value:[]},sunShadowCascade:{value:[]},directionalLights:{value:[],properties:{direction:{},color:{}}},directionalLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{}}},directionalShadowMatrix:{value:[]},spotLights:{value:[],properties:{color:{},position:{},direction:{},distance:{},coneCos:{},penumbraCos:{},decay:{}}},spotLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{}}},spotLightMap:{value:[]},spotLightMatrix:{value:[]},pointLights:{value:[],properties:{color:{},position:{},decay:{},distance:{}}},pointLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{},shadowCameraNear:{},shadowCameraFar:{}}},pointShadowMatrix:{value:[]},hemisphereLights:{value:[],properties:{direction:{},skyColor:{},groundColor:{}}},rectAreaLights:{value:[],properties:{color:{},position:{},width:{},height:{}}},ltc_1:{value:null},ltc_2:{value:null},probesSH:{value:null},probesMin:{value:new G},probesMax:{value:new G},probesResolution:{value:new G}},points:{diffuse:{value:new Ne(16777215)},opacity:{value:1},size:{value:1},scale:{value:1},map:{value:null},alphaMap:{value:null},alphaMapTransform:{value:new je},alphaTest:{value:0},uvTransform:{value:new je}},sprite:{diffuse:{value:new Ne(16777215)},opacity:{value:1},center:{value:new We(.5,.5)},rotation:{value:0},map:{value:null},mapTransform:{value:new je},alphaMap:{value:null},alphaMapTransform:{value:new je},alphaTest:{value:0}}},mi={basic:{uniforms:An([Te.common,Te.specularmap,Te.envmap,Te.aomap,Te.lightmap,Te.fog]),vertexShader:it.meshbasic_vert,fragmentShader:it.meshbasic_frag},lambert:{uniforms:An([Te.common,Te.specularmap,Te.envmap,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.fog,Te.lights,{emissive:{value:new Ne(0)},envMapIntensity:{value:1}}]),vertexShader:it.meshlambert_vert,fragmentShader:it.meshlambert_frag},phong:{uniforms:An([Te.common,Te.specularmap,Te.envmap,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.fog,Te.lights,{emissive:{value:new Ne(0)},specular:{value:new Ne(1118481)},shininess:{value:30},envMapIntensity:{value:1}}]),vertexShader:it.meshphong_vert,fragmentShader:it.meshphong_frag},standard:{uniforms:An([Te.common,Te.envmap,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.roughnessmap,Te.metalnessmap,Te.fog,Te.lights,{emissive:{value:new Ne(0)},roughness:{value:1},metalness:{value:0},envMapIntensity:{value:1}}]),vertexShader:it.meshphysical_vert,fragmentShader:it.meshphysical_frag},toon:{uniforms:An([Te.common,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.gradientmap,Te.fog,Te.lights,{emissive:{value:new Ne(0)}}]),vertexShader:it.meshtoon_vert,fragmentShader:it.meshtoon_frag},matcap:{uniforms:An([Te.common,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.fog,{matcap:{value:null}}]),vertexShader:it.meshmatcap_vert,fragmentShader:it.meshmatcap_frag},points:{uniforms:An([Te.points,Te.fog]),vertexShader:it.points_vert,fragmentShader:it.points_frag},dashed:{uniforms:An([Te.common,Te.fog,{scale:{value:1},dashSize:{value:1},totalSize:{value:2}}]),vertexShader:it.linedashed_vert,fragmentShader:it.linedashed_frag},depth:{uniforms:An([Te.common,Te.displacementmap]),vertexShader:it.depth_vert,fragmentShader:it.depth_frag},normal:{uniforms:An([Te.common,Te.bumpmap,Te.normalmap,Te.displacementmap,{opacity:{value:1}}]),vertexShader:it.meshnormal_vert,fragmentShader:it.meshnormal_frag},sprite:{uniforms:An([Te.sprite,Te.fog]),vertexShader:it.sprite_vert,fragmentShader:it.sprite_frag},background:{uniforms:{uvTransform:{value:new je},t2D:{value:null},backgroundIntensity:{value:1}},vertexShader:it.background_vert,fragmentShader:it.background_frag},backgroundCube:{uniforms:{envMap:{value:null},backgroundBlurriness:{value:0},backgroundIntensity:{value:1},backgroundRotation:{value:new je}},vertexShader:it.backgroundCube_vert,fragmentShader:it.backgroundCube_frag},cube:{uniforms:{tCube:{value:null},tFlip:{value:-1},opacity:{value:1}},vertexShader:it.cube_vert,fragmentShader:it.cube_frag},equirect:{uniforms:{tEquirect:{value:null}},vertexShader:it.equirect_vert,fragmentShader:it.equirect_frag},distance:{uniforms:An([Te.common,Te.displacementmap,{referencePosition:{value:new G},nearDistance:{value:1},farDistance:{value:1e3}}]),vertexShader:it.distance_vert,fragmentShader:it.distance_frag},shadow:{uniforms:An([Te.lights,Te.fog,{color:{value:new Ne(0)},opacity:{value:1}}]),vertexShader:it.shadow_vert,fragmentShader:it.shadow_frag}};mi.physical={uniforms:An([mi.standard.uniforms,{clearcoat:{value:0},clearcoatMap:{value:null},clearcoatMapTransform:{value:new je},clearcoatNormalMap:{value:null},clearcoatNormalMapTransform:{value:new je},clearcoatNormalScale:{value:new We(1,1)},clearcoatRoughness:{value:0},clearcoatRoughnessMap:{value:null},clearcoatRoughnessMapTransform:{value:new je},dispersion:{value:0},retroreflectivity:{value:0},iridescence:{value:0},iridescenceMap:{value:null},iridescenceMapTransform:{value:new je},iridescenceIOR:{value:1.3},iridescenceThicknessMinimum:{value:100},iridescenceThicknessMaximum:{value:400},iridescenceThicknessMap:{value:null},iridescenceThicknessMapTransform:{value:new je},sheen:{value:0},sheenColor:{value:new Ne(0)},sheenColorMap:{value:null},sheenColorMapTransform:{value:new je},sheenRoughness:{value:1},sheenRoughnessMap:{value:null},sheenRoughnessMapTransform:{value:new je},transmission:{value:0},transmissionMap:{value:null},transmissionMapTransform:{value:new je},transmissionSamplerSize:{value:new We},transmissionSamplerMap:{value:null},thickness:{value:0},thicknessMap:{value:null},thicknessMapTransform:{value:new je},attenuationDistance:{value:0},attenuationColor:{value:new Ne(0)},specularColor:{value:new Ne(1,1,1)},specularColorMap:{value:null},specularColorMapTransform:{value:new je},specularIntensity:{value:1},specularIntensityMap:{value:null},specularIntensityMapTransform:{value:new je},anisotropyVector:{value:new We},anisotropyMap:{value:null},anisotropyMapTransform:{value:new je}}]),vertexShader:it.meshphysical_vert,fragmentShader:it.meshphysical_frag};var vl={r:0,b:0,g:0},Zg=new wt,Bd=new je;Bd.set(-1,0,0,0,1,0,0,0,1);function Kg(n,e,t,i,r,s){let o=new Ne(0),a=r===!0?0:1,l,c,d=null,m=0,u=null;function g(M){let P=M.isScene===!0?M.background:null;if(P&&P.isTexture){let w=M.backgroundBlurriness>0;P=e.get(P,w)}return P}function _(M){let P=!1,w=g(M);w===null?f(o,a):w&&w.isColor&&(f(w,1),P=!0);let T=n.xr.getEnvironmentBlendMode();T==="additive"?t.buffers.color.setClear(0,0,0,1,s):T==="alpha-blend"&&t.buffers.color.setClear(0,0,0,0,s),(n.autoClear||P)&&(t.buffers.depth.setTest(!0),t.buffers.depth.setMask(!0),t.buffers.color.setMask(!0),n.clear(n.autoClearColor,n.autoClearDepth,n.autoClearStencil))}function v(M,P){let w=g(P);w&&(w.isCubeTexture||w.mapping===no)?(c===void 0&&(c=new rt(new Qe(1,1,1),new Tn({name:"BackgroundCubeMaterial",uniforms:vr(mi.backgroundCube.uniforms),vertexShader:mi.backgroundCube.vertexShader,fragmentShader:mi.backgroundCube.fragmentShader,side:fn,depthTest:!1,depthWrite:!1,fog:!1,allowOverride:!1})),c.geometry.deleteAttribute("normal"),c.geometry.deleteAttribute("uv"),c.onBeforeRender=function(T,E,U){this.matrixWorld.copyPosition(U.matrixWorld)},Object.defineProperty(c.material,"envMap",{get:function(){return this.uniforms.envMap.value}}),i.update(c)),c.material.uniforms.envMap.value=w,c.material.uniforms.backgroundBlurriness.value=P.backgroundBlurriness,c.material.uniforms.backgroundIntensity.value=P.backgroundIntensity,c.material.uniforms.backgroundRotation.value.setFromMatrix4(Zg.makeRotationFromEuler(P.backgroundRotation)).transpose(),w.isCubeTexture&&w.isRenderTargetTexture===!1&&c.material.uniforms.backgroundRotation.value.premultiply(Bd),c.material.toneMapped=ut.getTransfer(w.colorSpace)!==St,(d!==w||m!==w.version||u!==n.toneMapping)&&(c.material.needsUpdate=!0,d=w,m=w.version,u=n.toneMapping),c.layers.enableAll(),M.unshift(c,c.geometry,c.material,0,0,null)):w&&w.isTexture&&(l===void 0&&(l=new rt(new Vi(2,2),new Tn({name:"BackgroundMaterial",uniforms:vr(mi.background.uniforms),vertexShader:mi.background.vertexShader,fragmentShader:mi.background.fragmentShader,side:Ki,depthTest:!1,depthWrite:!1,fog:!1,allowOverride:!1})),l.geometry.deleteAttribute("normal"),Object.defineProperty(l.material,"map",{get:function(){return this.uniforms.t2D.value}}),i.update(l)),l.material.uniforms.t2D.value=w,l.material.uniforms.backgroundIntensity.value=P.backgroundIntensity,l.material.toneMapped=ut.getTransfer(w.colorSpace)!==St,w.matrixAutoUpdate===!0&&w.updateMatrix(),l.material.uniforms.uvTransform.value.copy(w.matrix),(d!==w||m!==w.version||u!==n.toneMapping)&&(l.material.needsUpdate=!0,d=w,m=w.version,u=n.toneMapping),l.layers.enableAll(),M.unshift(l,l.geometry,l.material,0,0,null))}function f(M,P){M.getRGB(vl,Jc(n)),t.buffers.color.setClear(vl.r,vl.g,vl.b,P,s)}function h(){c!==void 0&&(c.geometry.dispose(),c.material.dispose(),c=void 0),l!==void 0&&(l.geometry.dispose(),l.material.dispose(),l=void 0)}return{getClearColor:function(){return o},setClearColor:function(M,P=1){o.set(M),a=P,f(o,a)},getClearAlpha:function(){return a},setClearAlpha:function(M){a=M,f(o,a)},render:_,addToRenderList:v,dispose:h}}function Jg(n,e){let t=n.getParameter(n.MAX_VERTEX_ATTRIBS),i={},r=u(null),s=r,o=!1;function a(R,I,k,D,N){let H=!1,$=m(R,D,k,I);s!==$&&(s=$,c(s.object)),H=g(R,D,k,N),H&&_(R,D,k,N),N!==null&&e.update(N,n.ELEMENT_ARRAY_BUFFER),(H||o)&&(o=!1,w(R,I,k,D),N!==null&&n.bindBuffer(n.ELEMENT_ARRAY_BUFFER,e.get(N).buffer))}function l(){return n.createVertexArray()}function c(R){return n.bindVertexArray(R)}function d(R){return n.deleteVertexArray(R)}function m(R,I,k,D){let N=D.wireframe===!0,H=i[I.id];H===void 0&&(H={},i[I.id]=H);let $=R.isInstancedMesh===!0?R.id:0,ee=H[$];ee===void 0&&(ee={},H[$]=ee);let J=ee[k.id];J===void 0&&(J={},ee[k.id]=J);let ie=J[N];return ie===void 0&&(ie=u(l()),J[N]=ie),ie}function u(R){let I=[],k=[],D=[];for(let N=0;N<t;N++)I[N]=0,k[N]=0,D[N]=0;return{geometry:null,program:null,wireframe:!1,newAttributes:I,enabledAttributes:k,attributeDivisors:D,object:R,attributes:{},index:null}}function g(R,I,k,D){let N=s.attributes,H=I.attributes,$=0,ee=k.getAttributes();for(let J in ee)if(ee[J].location>=0){let ce=N[J],we=H[J];if(we===void 0&&(J==="instanceMatrix"&&R.instanceMatrix&&(we=R.instanceMatrix),J==="instanceColor"&&R.instanceColor&&(we=R.instanceColor)),ce===void 0||ce.attribute!==we||we&&ce.data!==we.data)return!0;$++}return s.attributesNum!==$||s.index!==D}function _(R,I,k,D){let N={},H=I.attributes,$=0,ee=k.getAttributes();for(let J in ee)if(ee[J].location>=0){let ce=H[J];ce===void 0&&(J==="instanceMatrix"&&R.instanceMatrix&&(ce=R.instanceMatrix),J==="instanceColor"&&R.instanceColor&&(ce=R.instanceColor));let we={};we.attribute=ce,ce&&ce.data&&(we.data=ce.data),N[J]=we,$++}s.attributes=N,s.attributesNum=$,s.index=D}function v(){let R=s.newAttributes;for(let I=0,k=R.length;I<k;I++)R[I]=0}function f(R){h(R,0)}function h(R,I){let k=s.newAttributes,D=s.enabledAttributes,N=s.attributeDivisors;k[R]=1,D[R]===0&&(n.enableVertexAttribArray(R),D[R]=1),N[R]!==I&&(n.vertexAttribDivisor(R,I),N[R]=I)}function M(){let R=s.newAttributes,I=s.enabledAttributes;for(let k=0,D=I.length;k<D;k++)I[k]!==R[k]&&(n.disableVertexAttribArray(k),I[k]=0)}function P(R,I,k,D,N,H,$){$===!0?n.vertexAttribIPointer(R,I,k,N,H):n.vertexAttribPointer(R,I,k,D,N,H)}function w(R,I,k,D){v();let N=D.attributes,H=k.getAttributes(),$=I.defaultAttributeValues;for(let ee in H){let J=H[ee];if(J.location>=0){let ie=N[ee];if(ie===void 0&&(ee==="instanceMatrix"&&R.instanceMatrix&&(ie=R.instanceMatrix),ee==="instanceColor"&&R.instanceColor&&(ie=R.instanceColor)),ie!==void 0){let ce=ie.normalized,we=ie.itemSize,pe=e.get(ie);if(pe===void 0)continue;let Pe=pe.buffer,Ve=pe.type,Oe=pe.bytesPerElement,oe=Ve===n.INT||Ve===n.UNSIGNED_INT||ie.gpuType===Ia;if(ie.isInterleavedBufferAttribute){let le=ie.data,be=le.stride,Ge=ie.offset;if(le.isInstancedInterleavedBuffer){for(let Me=0;Me<J.locationSize;Me++)h(J.location+Me,le.meshPerAttribute);R.isInstancedMesh!==!0&&D._maxInstanceCount===void 0&&(D._maxInstanceCount=le.meshPerAttribute*le.count)}else for(let Me=0;Me<J.locationSize;Me++)f(J.location+Me);n.bindBuffer(n.ARRAY_BUFFER,Pe);for(let Me=0;Me<J.locationSize;Me++)P(J.location+Me,we/J.locationSize,Ve,ce,be*Oe,(Ge+we/J.locationSize*Me)*Oe,oe)}else{if(ie.isInstancedBufferAttribute){for(let le=0;le<J.locationSize;le++)h(J.location+le,ie.meshPerAttribute);R.isInstancedMesh!==!0&&D._maxInstanceCount===void 0&&(D._maxInstanceCount=ie.meshPerAttribute*ie.count)}else for(let le=0;le<J.locationSize;le++)f(J.location+le);n.bindBuffer(n.ARRAY_BUFFER,Pe);for(let le=0;le<J.locationSize;le++)P(J.location+le,we/J.locationSize,Ve,ce,we*Oe,we/J.locationSize*le*Oe,oe)}}else if($!==void 0){let ce=$[ee];if(ce!==void 0)switch(ce.length){case 2:n.vertexAttrib2fv(J.location,ce);break;case 3:n.vertexAttrib3fv(J.location,ce);break;case 4:n.vertexAttrib4fv(J.location,ce);break;default:n.vertexAttrib1fv(J.location,ce)}}}}M()}function T(){S();for(let R in i){let I=i[R];for(let k in I){let D=I[k];for(let N in D){let H=D[N];for(let $ in H)d(H[$].object),delete H[$];delete D[N]}}delete i[R]}}function E(R){if(i[R.id]===void 0)return;let I=i[R.id];for(let k in I){let D=I[k];for(let N in D){let H=D[N];for(let $ in H)d(H[$].object),delete H[$];delete D[N]}}delete i[R.id]}function U(R){for(let I in i){let k=i[I];for(let D in k){let N=k[D];if(N[R.id]===void 0)continue;let H=N[R.id];for(let $ in H)d(H[$].object),delete H[$];delete N[R.id]}}}function p(R){for(let I in i){let k=i[I],D=R.isInstancedMesh===!0?R.id:0,N=k[D];if(N!==void 0){for(let H in N){let $=N[H];for(let ee in $)d($[ee].object),delete $[ee];delete N[H]}delete k[D],Object.keys(k).length===0&&delete i[I]}}}function S(){y(),o=!0,s!==r&&(s=r,c(s.object))}function y(){r.geometry=null,r.program=null,r.wireframe=!1}return{setup:a,reset:S,resetDefaultState:y,dispose:T,releaseStatesOfGeometry:E,releaseStatesOfObject:p,releaseStatesOfProgram:U,initAttributes:v,enableAttribute:f,disableUnusedAttributes:M}}function jg(n,e,t){let i;function r(l){i=l}function s(l,c){n.drawArrays(i,l,c),t.update(c,i,1)}function o(l,c,d){d!==0&&(n.drawArraysInstanced(i,l,c,d),t.update(c,i,d))}function a(l,c,d){if(d===0)return;e.get("WEBGL_multi_draw").multiDrawArraysWEBGL(i,l,0,c,0,d);let u=0;for(let g=0;g<d;g++)u+=c[g];t.update(u,i,1)}this.setMode=r,this.render=s,this.renderInstances=o,this.renderMultiDraw=a}function Qg(n,e,t,i){let r;function s(){if(r!==void 0)return r;if(e.has("EXT_texture_filter_anisotropic")===!0){let U=e.get("EXT_texture_filter_anisotropic");r=n.getParameter(U.MAX_TEXTURE_MAX_ANISOTROPY_EXT)}else r=0;return r}function o(U){return!(U!==Fn&&i.convert(U)!==n.getParameter(n.IMPLEMENTATION_COLOR_READ_FORMAT))}function a(U){let p=U===ri&&(e.has("EXT_color_buffer_half_float")||e.has("EXT_color_buffer_float"));return!(U!==Un&&U!==$n&&!p&&i.convert(U)!==n.getParameter(n.IMPLEMENTATION_COLOR_READ_TYPE))}function l(U){if(U==="highp"){if(n.getShaderPrecisionFormat(n.VERTEX_SHADER,n.HIGH_FLOAT).precision>0&&n.getShaderPrecisionFormat(n.FRAGMENT_SHADER,n.HIGH_FLOAT).precision>0)return"highp";U="mediump"}return U==="mediump"&&n.getShaderPrecisionFormat(n.VERTEX_SHADER,n.MEDIUM_FLOAT).precision>0&&n.getShaderPrecisionFormat(n.FRAGMENT_SHADER,n.MEDIUM_FLOAT).precision>0?"mediump":"lowp"}let c=t.precision!==void 0?t.precision:"highp",d=l(c);d!==c&&($e("WebGLRenderer:",c,"not supported, using",d,"instead."),c=d);let m=t.logarithmicDepthBuffer===!0,u=t.reversedDepthBuffer===!0&&e.has("EXT_clip_control");t.reversedDepthBuffer===!0&&u===!1&&$e("WebGLRenderer: Unable to use reversed depth buffer due to missing EXT_clip_control extension. Fallback to default depth buffer.");let g=n.getParameter(n.MAX_TEXTURE_IMAGE_UNITS),_=n.getParameter(n.MAX_VERTEX_TEXTURE_IMAGE_UNITS),v=n.getParameter(n.MAX_TEXTURE_SIZE),f=n.getParameter(n.MAX_CUBE_MAP_TEXTURE_SIZE),h=n.getParameter(n.MAX_VERTEX_ATTRIBS),M=n.getParameter(n.MAX_VERTEX_UNIFORM_VECTORS),P=n.getParameter(n.MAX_VARYING_VECTORS),w=n.getParameter(n.MAX_FRAGMENT_UNIFORM_VECTORS),T=n.getParameter(n.MAX_SAMPLES),E=n.getParameter(n.SAMPLES);return{isWebGL2:!0,getMaxAnisotropy:s,getMaxPrecision:l,textureFormatReadable:o,textureTypeReadable:a,precision:c,logarithmicDepthBuffer:m,reversedDepthBuffer:u,maxTextures:g,maxVertexTextures:_,maxTextureSize:v,maxCubemapSize:f,maxAttributes:h,maxVertexUniforms:M,maxVaryings:P,maxFragmentUniforms:w,maxSamples:T,samples:E}}function ex(n){let e=this,t=null,i=0,r=!1,s=!1,o=new Hn,a=new je,l={value:null,needsUpdate:!1};this.uniform=l,this.numPlanes=0,this.numIntersection=0,this.init=function(m,u){let g=m.length!==0||u||i!==0||r;return r=u,i=m.length,g},this.beginShadows=function(){s=!0,d(null)},this.endShadows=function(){s=!1},this.setGlobalState=function(m,u){t=d(m,u,0)},this.setState=function(m,u,g){let _=m.clippingPlanes,v=m.clipIntersection,f=m.clipShadows,h=n.get(m);if(!r||_===null||_.length===0||s&&!f)s?d(null):c();else{let M=s?0:i,P=M*4,w=h.clippingState||null;l.value=w,w=d(_,u,P,g);for(let T=0;T!==P;++T)w[T]=t[T];h.clippingState=w,this.numIntersection=v?this.numPlanes:0,this.numPlanes+=M}};function c(){l.value!==t&&(l.value=t,l.needsUpdate=i>0),e.numPlanes=i,e.numIntersection=0}function d(m,u,g,_){let v=m!==null?m.length:0,f=null;if(v!==0){if(f=l.value,_!==!0||f===null){let h=g+v*4,M=u.matrixWorldInverse;a.getNormalMatrix(M),(f===null||f.length<h)&&(f=new Float32Array(h));for(let P=0,w=g;P!==v;++P,w+=4)o.copy(m[P]).applyMatrix4(M,a),o.normal.toArray(f,w),f[w+3]=o.constant}l.value=f,l.needsUpdate=!0}return e.numPlanes=v,e.numIntersection=0,f}}var hs=4,tx=6,nx=20,ix=256,uo=new ns,xd=new Ne,tu=null,nu=0,iu=0,ru=!1,rx=new G,_r=new G,yl=class{constructor(e){this._renderer=e,this._pingPongRenderTarget=null,this._lodMax=0,this._cubeSize=0,this._sizeLods=[],this._lodMeshes=[],this._backgroundBox=null,this._cubemapMaterial=null,this._equirectMaterial=null,this._blurMaterial=null,this._ggxMaterial=null}fromScene(e,t=0,i=.1,r=100,s={}){let{size:o=256,position:a=rx}=s;tu=this._renderer.getRenderTarget(),nu=this._renderer.getActiveCubeFace(),iu=this._renderer.getActiveMipmapLevel(),ru=this._renderer.xr.enabled,this._renderer.xr.enabled=!1,this._setSize(o);let l=this._allocateTargets();return l.depthBuffer=!0,this._sceneToCubeUV(e,i,r,l,a),t>0&&this._blur(l,0,0,t),this._applyPMREM(l),this._cleanup(l),l}fromEquirectangular(e,t=null){return this._fromTexture(e,t)}fromCubemap(e,t=null){return this._fromTexture(e,t)}compileCubemapShader(){this._cubemapMaterial===null&&(this._cubemapMaterial=yd(),this._compileMaterial(this._cubemapMaterial))}compileEquirectangularShader(){this._equirectMaterial===null&&(this._equirectMaterial=_d(),this._compileMaterial(this._equirectMaterial))}dispose(){this._dispose(),this._cubemapMaterial!==null&&this._cubemapMaterial.dispose(),this._equirectMaterial!==null&&this._equirectMaterial.dispose(),this._backgroundBox!==null&&(this._backgroundBox.geometry.dispose(),this._backgroundBox.material.dispose())}_setSize(e){this._lodMax=Math.floor(Math.log2(e)),this._cubeSize=Math.pow(2,this._lodMax)}_dispose(){this._blurMaterial!==null&&this._blurMaterial.dispose(),this._ggxMaterial!==null&&this._ggxMaterial.dispose(),this._pingPongRenderTarget!==null&&this._pingPongRenderTarget.dispose();for(let e=0;e<this._lodMeshes.length;e++)this._lodMeshes[e].geometry.dispose()}_cleanup(e){this._renderer.setRenderTarget(tu,nu,iu),this._renderer.xr.enabled=ru,e.scissorTest=!1,us(e,0,0,e.width,e.height)}_fromTexture(e,t){e.mapping===Ji||e.mapping===xr?this._setSize(e.image.length===0?16:e.image[0].width||e.image[0].image.width):this._setSize(e.image.width/4),tu=this._renderer.getRenderTarget(),nu=this._renderer.getActiveCubeFace(),iu=this._renderer.getActiveMipmapLevel(),ru=this._renderer.xr.enabled,this._renderer.xr.enabled=!1;let i=t||this._allocateTargets();return this._textureToCubeUV(e,i),this._applyPMREM(i),this._cleanup(i),i}_allocateTargets(){let e=3*Math.max(this._cubeSize,112),t=4*this._cubeSize,i={magFilter:xn,minFilter:xn,generateMipmaps:!1,type:ri,format:Fn,colorSpace:Ps,depthBuffer:!1},r=vd(e,t,i);if(this._pingPongRenderTarget===null||this._pingPongRenderTarget.width!==e||this._pingPongRenderTarget.height!==t){this._pingPongRenderTarget!==null&&this._dispose(),this._pingPongRenderTarget=vd(e,t,i);let{_lodMax:s}=this;({lodMeshes:this._lodMeshes,sizeLods:this._sizeLods}=sx(s)),this._blurMaterial=ax(s,e,t),this._ggxMaterial=ox(s,e,t)}return r}_compileMaterial(e){let t=new rt(new $t,e);this._renderer.compile(t,uo)}_sceneToCubeUV(e,t,i,r,s){let l=new gn(90,1,t,i),c=[1,-1,1,1,1,1],d=[1,1,1,-1,-1,-1],m=this._renderer,u=m.autoClear,g=m.toneMapping;m.getClearColor(xd),m.toneMapping=ni,m.autoClear=!1,m.state.buffers.depth.getReversed()&&(m.setRenderTarget(r),m.clearDepth(),m.setRenderTarget(null)),this._backgroundBox===null&&(this._backgroundBox=new rt(new Qe,new ln({name:"PMREM.Background",side:fn,depthWrite:!1,depthTest:!1})));let v=this._backgroundBox,f=v.material,h=!1,M=e.background;M?M.isColor&&(f.color.copy(M),e.background=null,h=!0):(f.color.copy(xd),h=!0);for(let P=0;P<6;P++){let w=P%3;w===0?(l.up.set(0,c[P],0),l.position.set(s.x,s.y,s.z),l.lookAt(s.x+d[P],s.y,s.z)):w===1?(l.up.set(0,0,c[P]),l.position.set(s.x,s.y,s.z),l.lookAt(s.x,s.y+d[P],s.z)):(l.up.set(0,c[P],0),l.position.set(s.x,s.y,s.z),l.lookAt(s.x,s.y,s.z+d[P]));let T=this._cubeSize;us(r,w*T,P>2?T:0,T,T),m.setRenderTarget(r),h&&m.render(v,l),m.render(e,l)}m.toneMapping=g,m.autoClear=u,e.background=M}_textureToCubeUV(e,t){let i=this._renderer,r=e.mapping===Ji||e.mapping===xr;r?(this._cubemapMaterial===null&&(this._cubemapMaterial=yd()),this._cubemapMaterial.uniforms.flipEnvMap.value=e.isRenderTargetTexture===!1?-1:1):this._equirectMaterial===null&&(this._equirectMaterial=_d());let s=r?this._cubemapMaterial:this._equirectMaterial,o=this._lodMeshes[0];o.material=s;let a=s.uniforms;a.envMap.value=e;let l=this._cubeSize;us(t,0,0,3*l,2*l),i.setRenderTarget(t),i.render(o,uo)}_applyPMREM(e){let t=this._renderer,i=t.autoClear;t.autoClear=!1;let r=this._lodMeshes.length;for(let s=1;s<r;s++)this._applyGGXFilter(e,s-1,s);t.autoClear=i}_applyGGXFilter(e,t,i){let r=this._renderer,s=this._pingPongRenderTarget,o=this._ggxMaterial,a=this._lodMeshes[i];a.material=o;let l=o.uniforms,c=i/(this._lodMeshes.length-1),d=t/(this._lodMeshes.length-1),m=Math.sqrt(c*c-d*d),u=c*1.25,g=m*u,{_lodMax:_}=this,v=this._sizeLods[i],f=3*v*(i>_-hs?i-_+hs:0),h=4*(this._cubeSize-v);l.envMap.value=e.texture,l.roughness.value=g,l.mipInt.value=_-t,us(s,f,h,3*v,2*v),r.setRenderTarget(s),r.render(a,uo),l.envMap.value=s.texture,l.roughness.value=0,l.mipInt.value=_-i,us(e,f,h,3*v,2*v),r.setRenderTarget(e),r.render(a,uo)}_blur(e,t,i,r){let s=this._pingPongRenderTarget,o=Math.min(r,Math.PI)/Math.SQRT2;this._blurPass(e,s,t,i,o),this._blurPass(s,e,i,i,o)}_blurPass(e,t,i,r,s){let o=this._renderer,a=this._blurMaterial,l=this._lodMeshes[r];l.material=a;let c=a.uniforms;c.envMap.value=e.texture,c.sigma.value=s,c.mipInt.value=this._lodMax-i;let d=this._sizeLods[r],m=3*d*(r>this._lodMax-hs?r-this._lodMax+hs:0),u=4*(this._cubeSize-d);us(t,m,u,3*d,2*d),o.setRenderTarget(t),o.render(l,uo)}};function sx(n){let e=[],t=[],i=n,r=n-hs+1+tx;for(let s=0;s<r;s++){let o=Math.pow(2,i);e.push(o);let a=1/(o-2),l=-a,c=1+a,d=[l,l,c,l,c,c,l,l,c,c,l,c],m=6,u=6,g=3,_=new Float32Array(g*u*m),v=new Float32Array(g*u*m);for(let h=0;h<m;h++){let M=h%3*2/3-1,P=h>2?0:-1,w=[M,P,0,M+2/3,P,0,M+2/3,P+1,0,M,P,0,M+2/3,P+1,0,M,P+1,0];_.set(w,g*u*h);for(let T=0;T<u;T++){let E=d[T*2]*2-1,U=d[T*2+1]*2-1;h===0?_r.set(1,U,E):h===1?_r.set(-E,1,-U):h===2?_r.set(-E,U,1):h===3?_r.set(-1,U,-E):h===4?_r.set(-E,-1,U):_r.set(E,U,-1),_r.toArray(v,(h*u+T)*g)}}let f=new $t;f.setAttribute("position",new Pn(_,g)),f.setAttribute("outputDirection",new Pn(v,g)),t.push(new rt(f,null)),i>hs&&i--}return{lodMeshes:t,sizeLods:e}}function vd(n,e,t){let i=new kn(n,e,t);return i.texture.mapping=no,i.texture.name="PMREM.cubeUv",i.scissorTest=!0,i}function us(n,e,t,i,r){n.viewport.set(e,t,i,r),n.scissor.set(e,t,i,r)}function ox(n,e,t){return new Tn({name:"PMREMGGXConvolution",defines:{GGX_SAMPLES:ix,CUBEUV_TEXEL_WIDTH:1/e,CUBEUV_TEXEL_HEIGHT:1/t,CUBEUV_MAX_MIP:`${n}.0`},uniforms:{envMap:{value:null},roughness:{value:0},mipInt:{value:0}},vertexShader:Sl(),fragmentShader:`

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
		`,blending:fi,depthTest:!1,depthWrite:!1})}function ax(n,e,t){return new Tn({name:"SphericalGaussianBlur",defines:{SAMPLES:nx,CUBEUV_TEXEL_WIDTH:1/e,CUBEUV_TEXEL_HEIGHT:1/t,CUBEUV_MAX_MIP:`${n}.0`},uniforms:{envMap:{value:null},sigma:{value:0},mipInt:{value:0}},vertexShader:Sl(),fragmentShader:`

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
		`,blending:fi,depthTest:!1,depthWrite:!1})}function _d(){return new Tn({name:"EquirectangularToCubeUV",uniforms:{envMap:{value:null}},vertexShader:Sl(),fragmentShader:`

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
		`,blending:fi,depthTest:!1,depthWrite:!1})}function yd(){return new Tn({name:"CubemapToCubeUV",uniforms:{envMap:{value:null},flipEnvMap:{value:-1}},vertexShader:Sl(),fragmentShader:`

			precision mediump float;
			precision mediump int;

			uniform float flipEnvMap;

			varying vec3 vOutputDirection;

			uniform samplerCube envMap;

			void main() {

				gl_FragColor = textureCube( envMap, vec3( flipEnvMap * vOutputDirection.x, vOutputDirection.yz ) );

			}
		`,blending:fi,depthTest:!1,depthWrite:!1})}function Sl(){return`

		precision mediump float;
		precision mediump int;

		attribute vec3 outputDirection;

		varying vec3 vOutputDirection;

		void main() {

			vOutputDirection = outputDirection;
			gl_Position = vec4( position, 1.0 );

		}
	`}var bl=class extends kn{constructor(e=1,t={}){super(e,e,t),this.isWebGLCubeRenderTarget=!0;let i={width:e,height:e,depth:1},r=[i,i,i,i,i,i];this.texture=new Hs(r),this._setTextureOptions(t),this.texture.isRenderTargetTexture=!0}fromEquirectangularTexture(e,t){this.texture.type=t.type,this.texture.colorSpace=t.colorSpace,this.texture.generateMipmaps=t.generateMipmaps,this.texture.minFilter=t.minFilter,this.texture.magFilter=t.magFilter;let i={uniforms:{tEquirect:{value:null}},vertexShader:`

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
			`},r=new Qe(5,5,5),s=new Tn({name:"CubemapFromEquirect",uniforms:vr(i.uniforms),vertexShader:i.vertexShader,fragmentShader:i.fragmentShader,side:fn,blending:fi});s.uniforms.tEquirect.value=t;let o=new rt(r,s),a=t.minFilter;return t.minFilter===ji&&(t.minFilter=xn),new Ta(1,10,this).update(e,o),t.minFilter=a,o.geometry.dispose(),o.material.dispose(),this}clear(e,t=!0,i=!0,r=!0){let s=e.getRenderTarget();for(let o=0;o<6;o++)e.setRenderTarget(this,o),e.clear(t,i,r);e.setRenderTarget(s)}};function lx(n){let e=new WeakMap,t=new WeakMap,i=null;function r(u,g=!1){return u==null?null:g?o(u):s(u)}function s(u){if(u&&u.isTexture){let g=u.mapping;if(g===Ra||g===Ca)if(e.has(u)){let _=e.get(u).texture;return a(_,u.mapping)}else{let _=u.image;if(_&&_.height>0){let v=new bl(_.height);return v.fromEquirectangularTexture(n,u),e.set(u,v),u.addEventListener("dispose",c),a(v.texture,u.mapping)}else return null}}return u}function o(u){if(u&&u.isTexture){let g=u.mapping,_=g===Ra||g===Ca,v=g===Ji||g===xr;if(_||v){let f=t.get(u),h=f!==void 0?f.texture.pmremVersion:0;if(u.isRenderTargetTexture&&u.pmremVersion!==h)return i===null&&(i=new yl(n)),f=_?i.fromEquirectangular(u,f):i.fromCubemap(u,f),f.texture.pmremVersion=u.pmremVersion,t.set(u,f),f.texture;if(f!==void 0)return f.texture;{let M=u.image;return _&&M&&M.height>0||v&&M&&l(M)?(i===null&&(i=new yl(n)),f=_?i.fromEquirectangular(u):i.fromCubemap(u),f.texture.pmremVersion=u.pmremVersion,t.set(u,f),u.addEventListener("dispose",d),f.texture):null}}}return u}function a(u,g){return g===Ra?u.mapping=Ji:g===Ca&&(u.mapping=xr),u}function l(u){let g=0,_=6;for(let v=0;v<_;v++)u[v]!==void 0&&g++;return g===_}function c(u){let g=u.target;g.removeEventListener("dispose",c);let _=e.get(g);_!==void 0&&(e.delete(g),_.dispose())}function d(u){let g=u.target;g.removeEventListener("dispose",d);let _=t.get(g);_!==void 0&&(t.delete(g),_.dispose())}function m(){e=new WeakMap,t=new WeakMap,i!==null&&(i.dispose(),i=null)}return{get:r,dispose:m}}function cx(n){let e={};function t(i){if(e[i]!==void 0)return e[i];let r=n.getExtension(i);return e[i]=r,r}return{has:function(i){return t(i)!==null},init:function(){t("EXT_color_buffer_float"),t("WEBGL_clip_cull_distance"),t("OES_texture_float_linear"),t("EXT_color_buffer_half_float"),t("WEBGL_multisampled_render_to_texture"),t("WEBGL_render_shared_exponent")},get:function(i){let r=t(i);return r===null&&hr("WebGLRenderer: "+i+" extension not supported."),r}}}function ux(n,e,t,i){let r={},s=new WeakMap;function o(m){let u=m.target;u.index!==null&&e.remove(u.index);for(let _ in u.attributes)e.remove(u.attributes[_]);u.removeEventListener("dispose",o),delete r[u.id];let g=s.get(u);g&&(e.remove(g),s.delete(u)),i.releaseStatesOfGeometry(u),u.isInstancedBufferGeometry===!0&&delete u._maxInstanceCount,t.memory.geometries--}function a(m,u){return r[u.id]===!0||(u.addEventListener("dispose",o),r[u.id]=!0,t.memory.geometries++),u}function l(m){let u=m.attributes;for(let g in u)e.update(u[g],n.ARRAY_BUFFER)}function c(m){let u=[],g=m.index,_=m.attributes.position,v=0;if(_===void 0)return;if(g!==null){let M=g.array;v=g.version;for(let P=0,w=M.length;P<w;P+=3){let T=M[P+0],E=M[P+1],U=M[P+2];u.push(T,E,E,U,U,T)}}else{let M=_.array;v=_.version;for(let P=0,w=M.length/3-1;P<w;P+=3){let T=P+0,E=P+1,U=P+2;u.push(T,E,E,U,U,T)}}let f=new(_.count>=65535?Fs:Us)(u,1);f.version=v;let h=s.get(m);h&&e.remove(h),s.set(m,f)}function d(m){let u=s.get(m);if(u){let g=m.index;g!==null&&u.version<g.version&&c(m)}else c(m);return s.get(m)}return{get:a,update:l,getWireframeAttribute:d}}function hx(n,e,t){let i;function r(m){i=m}let s,o;function a(m){s=m.type,o=m.bytesPerElement}function l(m,u){n.drawElements(i,u,s,m*o),t.update(u,i,1)}function c(m,u,g){g!==0&&(n.drawElementsInstanced(i,u,s,m*o,g),t.update(u,i,g))}function d(m,u,g){if(g===0)return;e.get("WEBGL_multi_draw").multiDrawElementsWEBGL(i,u,0,s,m,0,g);let v=0;for(let f=0;f<g;f++)v+=u[f];t.update(v,i,1)}this.setMode=r,this.setIndex=a,this.render=l,this.renderInstances=c,this.renderMultiDraw=d}function dx(n){let e={geometries:0,textures:0},t={frame:0,calls:0,triangles:0,points:0,lines:0};function i(s,o,a){switch(t.calls++,o){case n.TRIANGLES:t.triangles+=a*(s/3);break;case n.LINES:t.lines+=a*(s/2);break;case n.LINE_STRIP:t.lines+=a*(s-1);break;case n.LINE_LOOP:t.lines+=a*s;break;case n.POINTS:t.points+=a*s;break;default:Ze("WebGLInfo: Unknown draw mode:",o);break}}function r(){t.calls=0,t.triangles=0,t.points=0,t.lines=0}return{memory:e,render:t,programs:null,autoReset:!0,reset:r,update:i}}function fx(n,e,t){let i=new WeakMap,r=new Bt;function s(o,a,l){let c=o.morphTargetInfluences,d=a.morphAttributes.position||a.morphAttributes.normal||a.morphAttributes.color,m=d!==void 0?d.length:0,u=i.get(a);if(u===void 0||u.count!==m){let S=function(){U.dispose(),i.delete(a),a.removeEventListener("dispose",S)};u!==void 0&&u.texture.dispose();let g=a.morphAttributes.position!==void 0,_=a.morphAttributes.normal!==void 0,v=a.morphAttributes.color!==void 0,f=a.morphAttributes.position||[],h=a.morphAttributes.normal||[],M=a.morphAttributes.color||[],P=0;g===!0&&(P=1),_===!0&&(P=2),v===!0&&(P=3);let w=a.attributes.position.count*P,T=1;w>e.maxTextureSize&&(T=Math.ceil(w/e.maxTextureSize),w=e.maxTextureSize);let E=new Float32Array(w*T*4*m),U=new ks(E,w,T,m);U.type=$n,U.needsUpdate=!0;let p=P*4;for(let y=0;y<m;y++){let R=f[y],I=h[y],k=M[y],D=w*T*4*y;for(let N=0;N<R.count;N++){let H=N*p;g===!0&&(r.fromBufferAttribute(R,N),E[D+H+0]=r.x,E[D+H+1]=r.y,E[D+H+2]=r.z,E[D+H+3]=0),_===!0&&(r.fromBufferAttribute(I,N),E[D+H+4]=r.x,E[D+H+5]=r.y,E[D+H+6]=r.z,E[D+H+7]=0),v===!0&&(r.fromBufferAttribute(k,N),E[D+H+8]=r.x,E[D+H+9]=r.y,E[D+H+10]=r.z,E[D+H+11]=k.itemSize===4?r.w:1)}}u={count:m,texture:U,size:new We(w,T)},i.set(a,u),a.addEventListener("dispose",S)}if(o.isInstancedMesh===!0&&o.morphTexture!==null)l.getUniforms().setValue(n,"morphTexture",o.morphTexture,t);else{let g=0;for(let v=0;v<c.length;v++)g+=c[v];let _=a.morphTargetsRelative?1:1-g;l.getUniforms().setValue(n,"morphTargetBaseInfluence",_),l.getUniforms().setValue(n,"morphTargetInfluences",c)}l.getUniforms().setValue(n,"morphTargetsTexture",u.texture,t),l.getUniforms().setValue(n,"morphTargetsTextureSize",u.size)}return{update:s}}function px(n,e,t,i,r){let s=new WeakMap;function o(c){let d=r.render.frame,m=c.geometry,u=e.get(c,m);if(s.get(u)!==d&&(e.update(u),s.set(u,d)),c.isInstancedMesh&&(c.hasEventListener("dispose",l)===!1&&c.addEventListener("dispose",l),s.get(c)!==d&&(t.update(c.instanceMatrix,n.ARRAY_BUFFER),c.instanceColor!==null&&t.update(c.instanceColor,n.ARRAY_BUFFER),s.set(c,d))),c.isSkinnedMesh){let g=c.skeleton;s.get(g)!==d&&(g.update(),s.set(g,d))}return u}function a(){s=new WeakMap}function l(c){let d=c.target;d.removeEventListener("dispose",l),i.releaseStatesOfObject(d),t.remove(d.instanceMatrix),d.instanceColor!==null&&t.remove(d.instanceColor)}return{update:o,dispose:a}}var mx={[kc]:"LINEAR_TONE_MAPPING",[Dc]:"REINHARD_TONE_MAPPING",[Nc]:"CINEON_TONE_MAPPING",[Uc]:"ACES_FILMIC_TONE_MAPPING",[Oc]:"AGX_TONE_MAPPING",[Bc]:"NEUTRAL_TONE_MAPPING",[Fc]:"CUSTOM_TONE_MAPPING"};function gx(n,e,t,i,r,s){let o=new kn(e,t,{type:n,depthBuffer:r,stencilBuffer:s,samples:i?4:0,storeMultisampledDepthBuffer:!1,storeMultisampledStencilBuffer:!1,resolveDepthBuffer:!1,resolveStencilBuffer:!1}),a=null,l=null,c=new $t;c.setAttribute("position",new bt([-1,3,0,-1,-1,0,3,-1,0],3)),c.setAttribute("uv",new bt([0,2,0,0,2,0],2));let d=new fa({uniforms:{tDiffuse:{value:null}},vertexShader:`
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
			}`,depthTest:!1,depthWrite:!1}),m=new rt(c,d),u=new ns(-1,1,1,-1,0,1),g=null,_=null,v=!1,f,h=null,M=[],P=!1;this.setSize=function(w,T){o.setSize(w,T),a!==null&&a.setSize(w,T),l!==null&&l.setSize(w,T);for(let E=0;E<M.length;E++){let U=M[E];U.setSize&&U.setSize(w,T)}},this.setEffects=function(w){M=w,P=M.length>0&&M[0].isRenderPass===!0;let T=o.width,E=o.height;M.length>0&&a===null&&(a=new kn(T,E,{type:ri,depthBuffer:!1,stencilBuffer:!1}),l=new kn(T,E,{type:ri,depthBuffer:!1,stencilBuffer:!1}));for(let U=0;U<M.length;U++){let p=M[U];p.setSize&&p.setSize(T,E)}},this.begin=function(w,T){if(v||w.toneMapping===ni&&M.length===0)return!1;if(h=T,T!==null){let E=T.width,U=T.height;(o.width!==E||o.height!==U)&&this.setSize(E,U)}return P===!1&&w.setRenderTarget(o),f=w.toneMapping,w.toneMapping=ni,!0},this.hasRenderPass=function(){return P},this.end=function(w,T){w.toneMapping=f,v=!0;let E=o,U=a;for(let p=0;p<M.length;p++){let S=M[p];S.enabled!==!1&&(S.render(w,U,E,T),S.needsSwap!==!1&&(E=U,U=U===a?l:a))}if(g!==w.outputColorSpace||_!==w.toneMapping){g=w.outputColorSpace,_=w.toneMapping,d.defines={},ut.getTransfer(g)===St&&(d.defines.SRGB_TRANSFER="");let p=mx[_];p&&(d.defines[p]=""),d.needsUpdate=!0}d.uniforms.tDiffuse.value=E.texture,w.setRenderTarget(h),w.render(m,u),h=null,v=!1},this.isCompositing=function(){return v},this.dispose=function(){o.dispose(),a!==null&&a.dispose(),l!==null&&l.dispose(),c.dispose(),d.dispose()}}var zd=new In,au=new Gi(1,1),Hd=new ks,Gd=new ha,Vd=new Hs,bd=[],Md=[],Sd=new Float32Array(16),wd=new Float32Array(9),Ed=new Float32Array(4);function fs(n,e,t){let i=n[0];if(i<=0||i>0)return n;let r=e*t,s=bd[r];if(s===void 0&&(s=new Float32Array(r),bd[r]=s),e!==0){i.toArray(s,0);for(let o=1,a=0;o!==e;++o)a+=t,n[o].toArray(s,a)}return s}function cn(n,e){if(n.length!==e.length)return!1;for(let t=0,i=n.length;t<i;t++)if(n[t]!==e[t])return!1;return!0}function un(n,e){for(let t=0,i=e.length;t<i;t++)n[t]=e[t]}function wl(n,e){let t=Md[e];t===void 0&&(t=new Int32Array(e),Md[e]=t);for(let i=0;i!==e;++i)t[i]=n.allocateTextureUnit();return t}function xx(n,e){let t=this.cache;t[0]!==e&&(n.uniform1f(this.addr,e),t[0]=e)}function vx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y)&&(n.uniform2f(this.addr,e.x,e.y),t[0]=e.x,t[1]=e.y);else{if(cn(t,e))return;n.uniform2fv(this.addr,e),un(t,e)}}function _x(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z)&&(n.uniform3f(this.addr,e.x,e.y,e.z),t[0]=e.x,t[1]=e.y,t[2]=e.z);else if(e.r!==void 0)(t[0]!==e.r||t[1]!==e.g||t[2]!==e.b)&&(n.uniform3f(this.addr,e.r,e.g,e.b),t[0]=e.r,t[1]=e.g,t[2]=e.b);else{if(cn(t,e))return;n.uniform3fv(this.addr,e),un(t,e)}}function yx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z||t[3]!==e.w)&&(n.uniform4f(this.addr,e.x,e.y,e.z,e.w),t[0]=e.x,t[1]=e.y,t[2]=e.z,t[3]=e.w);else{if(cn(t,e))return;n.uniform4fv(this.addr,e),un(t,e)}}function bx(n,e){let t=this.cache,i=e.elements;if(i===void 0){if(cn(t,e))return;n.uniformMatrix2fv(this.addr,!1,e),un(t,e)}else{if(cn(t,i))return;Ed.set(i),n.uniformMatrix2fv(this.addr,!1,Ed),un(t,i)}}function Mx(n,e){let t=this.cache,i=e.elements;if(i===void 0){if(cn(t,e))return;n.uniformMatrix3fv(this.addr,!1,e),un(t,e)}else{if(cn(t,i))return;wd.set(i),n.uniformMatrix3fv(this.addr,!1,wd),un(t,i)}}function Sx(n,e){let t=this.cache,i=e.elements;if(i===void 0){if(cn(t,e))return;n.uniformMatrix4fv(this.addr,!1,e),un(t,e)}else{if(cn(t,i))return;Sd.set(i),n.uniformMatrix4fv(this.addr,!1,Sd),un(t,i)}}function wx(n,e){let t=this.cache;t[0]!==e&&(n.uniform1i(this.addr,e),t[0]=e)}function Ex(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y)&&(n.uniform2i(this.addr,e.x,e.y),t[0]=e.x,t[1]=e.y);else{if(cn(t,e))return;n.uniform2iv(this.addr,e),un(t,e)}}function Tx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z)&&(n.uniform3i(this.addr,e.x,e.y,e.z),t[0]=e.x,t[1]=e.y,t[2]=e.z);else{if(cn(t,e))return;n.uniform3iv(this.addr,e),un(t,e)}}function Ax(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z||t[3]!==e.w)&&(n.uniform4i(this.addr,e.x,e.y,e.z,e.w),t[0]=e.x,t[1]=e.y,t[2]=e.z,t[3]=e.w);else{if(cn(t,e))return;n.uniform4iv(this.addr,e),un(t,e)}}function Rx(n,e){let t=this.cache;t[0]!==e&&(n.uniform1ui(this.addr,e),t[0]=e)}function Cx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y)&&(n.uniform2ui(this.addr,e.x,e.y),t[0]=e.x,t[1]=e.y);else{if(cn(t,e))return;n.uniform2uiv(this.addr,e),un(t,e)}}function Px(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z)&&(n.uniform3ui(this.addr,e.x,e.y,e.z),t[0]=e.x,t[1]=e.y,t[2]=e.z);else{if(cn(t,e))return;n.uniform3uiv(this.addr,e),un(t,e)}}function Ix(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z||t[3]!==e.w)&&(n.uniform4ui(this.addr,e.x,e.y,e.z,e.w),t[0]=e.x,t[1]=e.y,t[2]=e.z,t[3]=e.w);else{if(cn(t,e))return;n.uniform4uiv(this.addr,e),un(t,e)}}function Lx(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r);let s;this.type===n.SAMPLER_2D_SHADOW?(au.compareFunction=t.isReversedDepthBuffer()?gl:ml,s=au):s=zd,t.setTexture2D(e||s,r)}function kx(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r),t.setTexture3D(e||Gd,r)}function Dx(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r),t.setTextureCube(e||Vd,r)}function Nx(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r),t.setTexture2DArray(e||Hd,r)}function Ux(n){switch(n){case 5126:return xx;case 35664:return vx;case 35665:return _x;case 35666:return yx;case 35674:return bx;case 35675:return Mx;case 35676:return Sx;case 5124:case 35670:return wx;case 35667:case 35671:return Ex;case 35668:case 35672:return Tx;case 35669:case 35673:return Ax;case 5125:return Rx;case 36294:return Cx;case 36295:return Px;case 36296:return Ix;case 35678:case 36198:case 36298:case 36306:case 35682:return Lx;case 35679:case 36299:case 36307:return kx;case 35680:case 36300:case 36308:case 36293:return Dx;case 36289:case 36303:case 36311:case 36292:return Nx}}function Fx(n,e){n.uniform1fv(this.addr,e)}function Ox(n,e){let t=fs(e,this.size,2);n.uniform2fv(this.addr,t)}function Bx(n,e){let t=fs(e,this.size,3);n.uniform3fv(this.addr,t)}function zx(n,e){let t=fs(e,this.size,4);n.uniform4fv(this.addr,t)}function Hx(n,e){let t=fs(e,this.size,4);n.uniformMatrix2fv(this.addr,!1,t)}function Gx(n,e){let t=fs(e,this.size,9);n.uniformMatrix3fv(this.addr,!1,t)}function Vx(n,e){let t=fs(e,this.size,16);n.uniformMatrix4fv(this.addr,!1,t)}function Wx(n,e){n.uniform1iv(this.addr,e)}function Xx(n,e){n.uniform2iv(this.addr,e)}function qx(n,e){n.uniform3iv(this.addr,e)}function $x(n,e){n.uniform4iv(this.addr,e)}function Yx(n,e){n.uniform1uiv(this.addr,e)}function Zx(n,e){n.uniform2uiv(this.addr,e)}function Kx(n,e){n.uniform3uiv(this.addr,e)}function Jx(n,e){n.uniform4uiv(this.addr,e)}function jx(n,e,t){let i=this.cache,r=e.length,s=wl(t,r);cn(i,s)||(n.uniform1iv(this.addr,s),un(i,s));let o;this.type===n.SAMPLER_2D_SHADOW?o=au:o=zd;for(let a=0;a!==r;++a)t.setTexture2D(e[a]||o,s[a])}function Qx(n,e,t){let i=this.cache,r=e.length,s=wl(t,r);cn(i,s)||(n.uniform1iv(this.addr,s),un(i,s));for(let o=0;o!==r;++o)t.setTexture3D(e[o]||Gd,s[o])}function ev(n,e,t){let i=this.cache,r=e.length,s=wl(t,r);cn(i,s)||(n.uniform1iv(this.addr,s),un(i,s));for(let o=0;o!==r;++o)t.setTextureCube(e[o]||Vd,s[o])}function tv(n,e,t){let i=this.cache,r=e.length,s=wl(t,r);cn(i,s)||(n.uniform1iv(this.addr,s),un(i,s));for(let o=0;o!==r;++o)t.setTexture2DArray(e[o]||Hd,s[o])}function nv(n){switch(n){case 5126:return Fx;case 35664:return Ox;case 35665:return Bx;case 35666:return zx;case 35674:return Hx;case 35675:return Gx;case 35676:return Vx;case 5124:case 35670:return Wx;case 35667:case 35671:return Xx;case 35668:case 35672:return qx;case 35669:case 35673:return $x;case 5125:return Yx;case 36294:return Zx;case 36295:return Kx;case 36296:return Jx;case 35678:case 36198:case 36298:case 36306:case 35682:return jx;case 35679:case 36299:case 36307:return Qx;case 35680:case 36300:case 36308:case 36293:return ev;case 36289:case 36303:case 36311:case 36292:return tv}}var lu=class{constructor(e,t,i){this.id=e,this.addr=i,this.cache=[],this.type=t.type,this.setValue=Ux(t.type)}},cu=class{constructor(e,t,i){this.id=e,this.addr=i,this.cache=[],this.type=t.type,this.size=t.size,this.setValue=nv(t.type)}},uu=class{constructor(e){this.id=e,this.seq=[],this.map={}}setValue(e,t,i){let r=this.seq;for(let s=0,o=r.length;s!==o;++s){let a=r[s];a.setValue(e,t[a.id],i)}}},su=/(\w+)(\])?(\[|\.)?/g;function Td(n,e){n.seq.push(e),n.map[e.id]=e}function iv(n,e,t){let i=n.name,r=i.length;for(su.lastIndex=0;;){let s=su.exec(i),o=su.lastIndex,a=s[1],l=s[2]==="]",c=s[3];if(l&&(a=a|0),c===void 0||c==="["&&o+2===r){Td(t,c===void 0?new lu(a,n,e):new cu(a,n,e));break}else{let m=t.map[a];m===void 0&&(m=new uu(a),Td(t,m)),t=m}}}var ds=class{constructor(e,t){this.seq=[],this.map={};let i=e.getProgramParameter(t,e.ACTIVE_UNIFORMS);for(let o=0;o<i;++o){let a=e.getActiveUniform(t,o),l=e.getUniformLocation(t,a.name);iv(a,l,this)}let r=[],s=[];for(let o of this.seq)o.type===e.SAMPLER_2D_SHADOW||o.type===e.SAMPLER_CUBE_SHADOW||o.type===e.SAMPLER_2D_ARRAY_SHADOW?r.push(o):s.push(o);r.length>0&&(this.seq=r.concat(s))}setValue(e,t,i,r){let s=this.map[t];s!==void 0&&s.setValue(e,i,r)}setOptional(e,t,i){let r=t[i];r!==void 0&&this.setValue(e,i,r)}static upload(e,t,i,r){for(let s=0,o=t.length;s!==o;++s){let a=t[s],l=i[a.id];l.needsUpdate!==!1&&a.setValue(e,l.value,r)}}static seqWithValue(e,t){let i=[];for(let r=0,s=e.length;r!==s;++r){let o=e[r];o.id in t&&i.push(o)}return i}};function Ad(n,e,t){let i=n.createShader(e);return n.shaderSource(i,t),n.compileShader(i),i}var rv=37297,sv=0;function ov(n,e){let t=n.split(`
`),i=[],r=Math.max(e-6,0),s=Math.min(e+6,t.length);for(let o=r;o<s;o++){let a=o+1;i.push(`${a===e?">":" "} ${a}: ${t[o]}`)}return i.join(`
`)}var Rd=new je;function av(n){ut._getMatrix(Rd,ut.workingColorSpace,n);let e=`mat3( ${Rd.elements.map(t=>t.toFixed(4))} )`;switch(ut.getTransfer(n)){case Is:return[e,"LinearTransferOETF"];case St:return[e,"sRGBTransferOETF"];default:return $e("WebGLProgram: Unsupported color space: ",n),[e,"LinearTransferOETF"]}}function Cd(n,e,t){let i=n.getShaderParameter(e,n.COMPILE_STATUS),s=(n.getShaderInfoLog(e)||"").trim();if(i&&s==="")return"";let o=/ERROR: 0:(\d+)/.exec(s);if(o){let a=parseInt(o[1]);return t.toUpperCase()+`

`+s+`

`+ov(n.getShaderSource(e),a)}else return s}function lv(n,e){let t=av(e);return[`vec4 ${n}( vec4 value ) {`,`	return ${t[1]}( vec4( value.rgb * ${t[0]}, value.a ) );`,"}"].join(`
`)}var cv={[kc]:"Linear",[Dc]:"Reinhard",[Nc]:"Cineon",[Uc]:"ACESFilmic",[Oc]:"AgX",[Bc]:"Neutral",[Fc]:"Custom"};function uv(n,e){let t=cv[e];return t===void 0?($e("WebGLProgram: Unsupported toneMapping:",e),"vec3 "+n+"( vec3 color ) { return LinearToneMapping( color ); }"):"vec3 "+n+"( vec3 color ) { return "+t+"ToneMapping( color ); }"}var _l=new G;function hv(){ut.getLuminanceCoefficients(_l);let n=_l.x.toFixed(4),e=_l.y.toFixed(4),t=_l.z.toFixed(4);return["float luminance( const in vec3 rgb ) {",`	const vec3 weights = vec3( ${n}, ${e}, ${t} );`,"	return dot( weights, rgb );","}"].join(`
`)}function dv(n){return[n.extensionClipCullDistance?"#extension GL_ANGLE_clip_cull_distance : require":"",n.extensionMultiDraw?"#extension GL_ANGLE_multi_draw : require":""].filter(fo).join(`
`)}function fv(n){let e=[];for(let t in n){let i=n[t];i!==!1&&e.push("#define "+t+" "+i)}return e.join(`
`)}function pv(n,e){let t={},i=n.getProgramParameter(e,n.ACTIVE_ATTRIBUTES);for(let r=0;r<i;r++){let s=n.getActiveAttrib(e,r),o=s.name,a=1;s.type===n.FLOAT_MAT2&&(a=2),s.type===n.FLOAT_MAT3&&(a=3),s.type===n.FLOAT_MAT4&&(a=4),t[o]={type:s.type,location:n.getAttribLocation(e,o),locationSize:a}}return t}function fo(n){return n!==""}function Pd(n,e){let t=e.numSpotLightShadows+e.numSpotLightMaps-e.numSpotLightShadowsWithMaps;return n.replace(/NUM_SUN_LIGHTS/g,e.numSunLights).replace(/NUM_DIR_LIGHTS/g,e.numDirLights).replace(/NUM_SPOT_LIGHTS/g,e.numSpotLights).replace(/NUM_SPOT_LIGHT_MAPS/g,e.numSpotLightMaps).replace(/NUM_SPOT_LIGHT_COORDS/g,t).replace(/NUM_RECT_AREA_LIGHTS/g,e.numRectAreaLights).replace(/NUM_POINT_LIGHTS/g,e.numPointLights).replace(/NUM_HEMI_LIGHTS/g,e.numHemiLights).replace(/NUM_SUN_LIGHT_SHADOWS/g,e.numSunLightShadows).replace(/NUM_DIR_LIGHT_SHADOWS/g,e.numDirLightShadows).replace(/NUM_SPOT_LIGHT_SHADOWS_WITH_MAPS/g,e.numSpotLightShadowsWithMaps).replace(/NUM_SPOT_LIGHT_SHADOWS/g,e.numSpotLightShadows).replace(/NUM_POINT_LIGHT_SHADOWS/g,e.numPointLightShadows)}function Id(n,e){return n.replace(/NUM_CLIPPING_PLANES/g,e.numClippingPlanes).replace(/UNION_CLIPPING_PLANES/g,e.numClippingPlanes-e.numClipIntersection)}var mv=/^[ \t]*#include +<([\w\d./]+)>/gm;function hu(n){return n.replace(mv,xv)}var gv=new Map;function xv(n,e){let t=it[e];if(t===void 0){let i=gv.get(e);if(i!==void 0)t=it[i],$e('WebGLRenderer: Shader chunk "%s" has been deprecated. Use "%s" instead.',e,i);else throw new Error("THREE.WebGLProgram: Can not resolve #include <"+e+">")}return hu(t)}var vv=/#pragma unroll_loop_start\s+for\s*\(\s*int\s+i\s*=\s*(\d+)\s*;\s*i\s*<\s*(\d+)\s*;\s*i\s*\+\+\s*\)\s*{([\s\S]+?)}\s+#pragma unroll_loop_end/g;function Ld(n){return n.replace(vv,_v)}function _v(n,e,t,i){let r="";for(let s=parseInt(e);s<parseInt(t);s++)r+=i.replace(/\[\s*i\s*\]/g,"[ "+s+" ]").replace(/UNROLLED_LOOP_INDEX/g,s);return r}function kd(n){let e=`precision ${n.precision} float;
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
#define LOW_PRECISION`),e}var yv={[mr]:"SHADOWMAP_TYPE_PCF",[rs]:"SHADOWMAP_TYPE_VSM"};function bv(n){return yv[n.shadowMapType]||"SHADOWMAP_TYPE_BASIC"}var Mv={[Ji]:"ENVMAP_TYPE_CUBE",[xr]:"ENVMAP_TYPE_CUBE",[no]:"ENVMAP_TYPE_CUBE_UV"};function Sv(n){return n.envMap===!1?"ENVMAP_TYPE_CUBE":Mv[n.envMapMode]||"ENVMAP_TYPE_CUBE"}var wv={[xr]:"ENVMAP_MODE_REFRACTION"};function Ev(n){return n.envMap===!1?"ENVMAP_MODE_REFLECTION":wv[n.envMapMode]||"ENVMAP_MODE_REFLECTION"}var Tv={[Lc]:"ENVMAP_BLENDING_MULTIPLY",[Jh]:"ENVMAP_BLENDING_MIX",[jh]:"ENVMAP_BLENDING_ADD"};function Av(n){return n.envMap===!1?"ENVMAP_BLENDING_NONE":Tv[n.combine]||"ENVMAP_BLENDING_NONE"}function Rv(n){let e=n.envMapCubeUVHeight;if(e===null)return null;let t=Math.log2(e)-2,i=1/e;return{texelWidth:1/(3*Math.max(Math.pow(2,t),112)),texelHeight:i,maxMip:t}}function Cv(n,e,t,i){let r=n.getContext(),s=t.defines,o=t.vertexShader,a=t.fragmentShader,l=bv(t),c=Sv(t),d=Ev(t),m=Av(t),u=Rv(t),g=dv(t),_=fv(s),v=r.createProgram(),f,h,M=t.glslVersion?"#version "+t.glslVersion+`
`:"";t.isRawShaderMaterial?(f=["#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,_].filter(fo).join(`
`),f.length>0&&(f+=`
`),h=["#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,_].filter(fo).join(`
`),h.length>0&&(h+=`
`)):(f=[kd(t),"#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,_,t.extensionClipCullDistance?"#define USE_CLIP_DISTANCE":"",t.batching?"#define USE_BATCHING":"",t.batchingColor?"#define USE_BATCHING_COLOR":"",t.instancing?"#define USE_INSTANCING":"",t.instancingColor?"#define USE_INSTANCING_COLOR":"",t.instancingMorph?"#define USE_INSTANCING_MORPH":"",t.useFog&&t.fog?"#define USE_FOG":"",t.useFog&&t.fogExp2?"#define FOG_EXP2":"",t.map?"#define USE_MAP":"",t.envMap?"#define USE_ENVMAP":"",t.envMap?"#define "+d:"",t.lightMap?"#define USE_LIGHTMAP":"",t.aoMap?"#define USE_AOMAP":"",t.bumpMap?"#define USE_BUMPMAP":"",t.normalMap?"#define USE_NORMALMAP":"",t.normalMapObjectSpace?"#define USE_NORMALMAP_OBJECTSPACE":"",t.normalMapTangentSpace?"#define USE_NORMALMAP_TANGENTSPACE":"",t.displacementMap?"#define USE_DISPLACEMENTMAP":"",t.emissiveMap?"#define USE_EMISSIVEMAP":"",t.anisotropy?"#define USE_ANISOTROPY":"",t.anisotropyMap?"#define USE_ANISOTROPYMAP":"",t.clearcoatMap?"#define USE_CLEARCOATMAP":"",t.clearcoatRoughnessMap?"#define USE_CLEARCOAT_ROUGHNESSMAP":"",t.clearcoatNormalMap?"#define USE_CLEARCOAT_NORMALMAP":"",t.iridescenceMap?"#define USE_IRIDESCENCEMAP":"",t.iridescenceThicknessMap?"#define USE_IRIDESCENCE_THICKNESSMAP":"",t.specularMap?"#define USE_SPECULARMAP":"",t.specularColorMap?"#define USE_SPECULAR_COLORMAP":"",t.specularIntensityMap?"#define USE_SPECULAR_INTENSITYMAP":"",t.roughnessMap?"#define USE_ROUGHNESSMAP":"",t.metalnessMap?"#define USE_METALNESSMAP":"",t.alphaMap?"#define USE_ALPHAMAP":"",t.alphaHash?"#define USE_ALPHAHASH":"",t.transmission?"#define USE_TRANSMISSION":"",t.transmissionMap?"#define USE_TRANSMISSIONMAP":"",t.thicknessMap?"#define USE_THICKNESSMAP":"",t.sheenColorMap?"#define USE_SHEEN_COLORMAP":"",t.sheenRoughnessMap?"#define USE_SHEEN_ROUGHNESSMAP":"",t.mapUv?"#define MAP_UV "+t.mapUv:"",t.alphaMapUv?"#define ALPHAMAP_UV "+t.alphaMapUv:"",t.lightMapUv?"#define LIGHTMAP_UV "+t.lightMapUv:"",t.aoMapUv?"#define AOMAP_UV "+t.aoMapUv:"",t.emissiveMapUv?"#define EMISSIVEMAP_UV "+t.emissiveMapUv:"",t.bumpMapUv?"#define BUMPMAP_UV "+t.bumpMapUv:"",t.normalMapUv?"#define NORMALMAP_UV "+t.normalMapUv:"",t.displacementMapUv?"#define DISPLACEMENTMAP_UV "+t.displacementMapUv:"",t.metalnessMapUv?"#define METALNESSMAP_UV "+t.metalnessMapUv:"",t.roughnessMapUv?"#define ROUGHNESSMAP_UV "+t.roughnessMapUv:"",t.anisotropyMapUv?"#define ANISOTROPYMAP_UV "+t.anisotropyMapUv:"",t.clearcoatMapUv?"#define CLEARCOATMAP_UV "+t.clearcoatMapUv:"",t.clearcoatNormalMapUv?"#define CLEARCOAT_NORMALMAP_UV "+t.clearcoatNormalMapUv:"",t.clearcoatRoughnessMapUv?"#define CLEARCOAT_ROUGHNESSMAP_UV "+t.clearcoatRoughnessMapUv:"",t.iridescenceMapUv?"#define IRIDESCENCEMAP_UV "+t.iridescenceMapUv:"",t.iridescenceThicknessMapUv?"#define IRIDESCENCE_THICKNESSMAP_UV "+t.iridescenceThicknessMapUv:"",t.sheenColorMapUv?"#define SHEEN_COLORMAP_UV "+t.sheenColorMapUv:"",t.sheenRoughnessMapUv?"#define SHEEN_ROUGHNESSMAP_UV "+t.sheenRoughnessMapUv:"",t.specularMapUv?"#define SPECULARMAP_UV "+t.specularMapUv:"",t.specularColorMapUv?"#define SPECULAR_COLORMAP_UV "+t.specularColorMapUv:"",t.specularIntensityMapUv?"#define SPECULAR_INTENSITYMAP_UV "+t.specularIntensityMapUv:"",t.transmissionMapUv?"#define TRANSMISSIONMAP_UV "+t.transmissionMapUv:"",t.thicknessMapUv?"#define THICKNESSMAP_UV "+t.thicknessMapUv:"",t.vertexTangents&&t.flatShading===!1?"#define USE_TANGENT":"",t.vertexNormals?"#define HAS_NORMAL":"",t.vertexColors?"#define USE_COLOR":"",t.vertexAlphas?"#define USE_COLOR_ALPHA":"",t.vertexUv1s?"#define USE_UV1":"",t.vertexUv2s?"#define USE_UV2":"",t.vertexUv3s?"#define USE_UV3":"",t.pointsUvs?"#define USE_POINTS_UV":"",t.flatShading?"#define FLAT_SHADED":"",t.skinning?"#define USE_SKINNING":"",t.morphTargets?"#define USE_MORPHTARGETS":"",t.morphNormals&&t.flatShading===!1?"#define USE_MORPHNORMALS":"",t.morphColors?"#define USE_MORPHCOLORS":"",t.morphTargetsCount>0?"#define MORPHTARGETS_TEXTURE_STRIDE "+t.morphTextureStride:"",t.morphTargetsCount>0?"#define MORPHTARGETS_COUNT "+t.morphTargetsCount:"",t.doubleSided?"#define DOUBLE_SIDED":"",t.flipSided?"#define FLIP_SIDED":"",t.shadowMapEnabled?"#define USE_SHADOWMAP":"",t.shadowMapEnabled?"#define "+l:"",t.sizeAttenuation?"#define USE_SIZEATTENUATION":"",t.numLightProbes>0?"#define USE_LIGHT_PROBES":"",t.logarithmicDepthBuffer?"#define USE_LOGARITHMIC_DEPTH_BUFFER":"",t.reversedDepthBuffer?"#define USE_REVERSED_DEPTH_BUFFER":"","uniform mat4 modelMatrix;","uniform mat4 modelViewMatrix;","uniform mat4 projectionMatrix;","uniform mat4 viewMatrix;","uniform mat3 normalMatrix;","uniform vec3 cameraPosition;","uniform bool isOrthographic;","#ifdef USE_INSTANCING","	attribute mat4 instanceMatrix;","#endif","#ifdef USE_INSTANCING_COLOR","	attribute vec3 instanceColor;","#endif","#ifdef USE_INSTANCING_MORPH","	uniform sampler2D morphTexture;","#endif","attribute vec3 position;","attribute vec3 normal;","attribute vec2 uv;","#ifdef USE_UV1","	attribute vec2 uv1;","#endif","#ifdef USE_UV2","	attribute vec2 uv2;","#endif","#ifdef USE_UV3","	attribute vec2 uv3;","#endif","#ifdef USE_TANGENT","	attribute vec4 tangent;","#endif","#if defined( USE_COLOR_ALPHA )","	attribute vec4 color;","#elif defined( USE_COLOR )","	attribute vec3 color;","#endif","#ifdef USE_SKINNING","	attribute vec4 skinIndex;","	attribute vec4 skinWeight;","#endif",`
`].filter(fo).join(`
`),h=[kd(t),"#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,_,t.useFog&&t.fog?"#define USE_FOG":"",t.useFog&&t.fogExp2?"#define FOG_EXP2":"",t.alphaToCoverage?"#define ALPHA_TO_COVERAGE":"",t.map?"#define USE_MAP":"",t.matcap?"#define USE_MATCAP":"",t.envMap?"#define USE_ENVMAP":"",t.envMap?"#define "+c:"",t.envMap?"#define "+d:"",t.envMap?"#define "+m:"",u?"#define CUBEUV_TEXEL_WIDTH "+u.texelWidth:"",u?"#define CUBEUV_TEXEL_HEIGHT "+u.texelHeight:"",u?"#define CUBEUV_MAX_MIP "+u.maxMip+".0":"",t.lightMap?"#define USE_LIGHTMAP":"",t.aoMap?"#define USE_AOMAP":"",t.bumpMap?"#define USE_BUMPMAP":"",t.normalMap?"#define USE_NORMALMAP":"",t.normalMapObjectSpace?"#define USE_NORMALMAP_OBJECTSPACE":"",t.normalMapTangentSpace?"#define USE_NORMALMAP_TANGENTSPACE":"",t.packedNormalMap?"#define USE_PACKED_NORMALMAP":"",t.emissiveMap?"#define USE_EMISSIVEMAP":"",t.anisotropy?"#define USE_ANISOTROPY":"",t.anisotropyMap?"#define USE_ANISOTROPYMAP":"",t.clearcoat?"#define USE_CLEARCOAT":"",t.clearcoatMap?"#define USE_CLEARCOATMAP":"",t.clearcoatRoughnessMap?"#define USE_CLEARCOAT_ROUGHNESSMAP":"",t.clearcoatNormalMap?"#define USE_CLEARCOAT_NORMALMAP":"",t.dispersion?"#define USE_DISPERSION":"",t.retroreflection?"#define USE_RETROREFLECTION":"",t.iridescence?"#define USE_IRIDESCENCE":"",t.iridescenceMap?"#define USE_IRIDESCENCEMAP":"",t.iridescenceThicknessMap?"#define USE_IRIDESCENCE_THICKNESSMAP":"",t.specularMap?"#define USE_SPECULARMAP":"",t.specularColorMap?"#define USE_SPECULAR_COLORMAP":"",t.specularIntensityMap?"#define USE_SPECULAR_INTENSITYMAP":"",t.roughnessMap?"#define USE_ROUGHNESSMAP":"",t.metalnessMap?"#define USE_METALNESSMAP":"",t.alphaMap?"#define USE_ALPHAMAP":"",t.alphaTest?"#define USE_ALPHATEST":"",t.alphaHash?"#define USE_ALPHAHASH":"",t.sheen?"#define USE_SHEEN":"",t.sheenColorMap?"#define USE_SHEEN_COLORMAP":"",t.sheenRoughnessMap?"#define USE_SHEEN_ROUGHNESSMAP":"",t.transmission?"#define USE_TRANSMISSION":"",t.transmissionMap?"#define USE_TRANSMISSIONMAP":"",t.thicknessMap?"#define USE_THICKNESSMAP":"",t.vertexTangents&&t.flatShading===!1?"#define USE_TANGENT":"",t.vertexColors||t.instancingColor?"#define USE_COLOR":"",t.vertexAlphas||t.batchingColor?"#define USE_COLOR_ALPHA":"",t.vertexUv1s?"#define USE_UV1":"",t.vertexUv2s?"#define USE_UV2":"",t.vertexUv3s?"#define USE_UV3":"",t.pointsUvs?"#define USE_POINTS_UV":"",t.gradientMap?"#define USE_GRADIENTMAP":"",t.flatShading?"#define FLAT_SHADED":"",t.doubleSided?"#define DOUBLE_SIDED":"",t.flipSided?"#define FLIP_SIDED":"",t.shadowMapEnabled?"#define USE_SHADOWMAP":"",t.shadowMapEnabled?"#define "+l:"",t.premultipliedAlpha?"#define PREMULTIPLIED_ALPHA":"",t.numLightProbes>0?"#define USE_LIGHT_PROBES":"",t.numLightProbeGrids>0?"#define USE_LIGHT_PROBES_GRID":"",t.decodeVideoTexture?"#define DECODE_VIDEO_TEXTURE":"",t.decodeVideoTextureEmissive?"#define DECODE_VIDEO_TEXTURE_EMISSIVE":"",t.logarithmicDepthBuffer?"#define USE_LOGARITHMIC_DEPTH_BUFFER":"",t.reversedDepthBuffer?"#define USE_REVERSED_DEPTH_BUFFER":"","uniform mat4 viewMatrix;","uniform vec3 cameraPosition;","uniform bool isOrthographic;",t.toneMapping!==ni?"#define TONE_MAPPING":"",t.toneMapping!==ni?it.tonemapping_pars_fragment:"",t.toneMapping!==ni?uv("toneMapping",t.toneMapping):"",t.dithering?"#define DITHERING":"",t.opaque?"#define OPAQUE":"",it.colorspace_pars_fragment,lv("linearToOutputTexel",t.outputColorSpace),hv(),t.useDepthPacking?"#define DEPTH_PACKING "+t.depthPacking:"",`
`].filter(fo).join(`
`)),o=hu(o),o=Pd(o,t),o=Id(o,t),a=hu(a),a=Pd(a,t),a=Id(a,t),o=Ld(o),a=Ld(a),t.isRawShaderMaterial!==!0&&(M=`#version 300 es
`,f=[g,"#define attribute in","#define varying out","#define texture2D texture"].join(`
`)+`
`+f,h=["#define varying in",t.glslVersion===$c?"":"layout(location = 0) out highp vec4 pc_fragColor;",t.glslVersion===$c?"":"#define gl_FragColor pc_fragColor","#define gl_FragDepthEXT gl_FragDepth","#define texture2D texture","#define textureCube texture","#define texture2DProj textureProj","#define texture2DLodEXT textureLod","#define texture2DProjLodEXT textureProjLod","#define textureCubeLodEXT textureLod","#define texture2DGradEXT textureGrad","#define texture2DProjGradEXT textureProjGrad","#define textureCubeGradEXT textureGrad"].join(`
`)+`
`+h);let P=M+f+o,w=M+h+a,T=Ad(r,r.VERTEX_SHADER,P),E=Ad(r,r.FRAGMENT_SHADER,w);r.attachShader(v,T),r.attachShader(v,E),t.index0AttributeName!==void 0?r.bindAttribLocation(v,0,t.index0AttributeName):t.hasPositionAttribute===!0&&r.bindAttribLocation(v,0,"position"),r.linkProgram(v);function U(R){if(n.debug.checkShaderErrors){let I=r.getProgramInfoLog(v)||"",k=r.getShaderInfoLog(T)||"",D=r.getShaderInfoLog(E)||"",N=I.trim(),H=k.trim(),$=D.trim(),ee=!0,J=!0;if(r.getProgramParameter(v,r.LINK_STATUS)===!1)if(ee=!1,typeof n.debug.onShaderError=="function")n.debug.onShaderError(r,v,T,E);else{let ie=Cd(r,T,"vertex"),ce=Cd(r,E,"fragment");Ze("WebGLProgram: Shader Error "+r.getError()+" - VALIDATE_STATUS "+r.getProgramParameter(v,r.VALIDATE_STATUS)+`

Material Name: `+R.name+`
Material Type: `+R.type+`

Program Info Log: `+N+`
`+ie+`
`+ce)}else N!==""?$e("WebGLProgram: Program Info Log:",N):(H===""||$==="")&&(J=!1);J&&(R.diagnostics={runnable:ee,programLog:N,vertexShader:{log:H,prefix:f},fragmentShader:{log:$,prefix:h}})}r.deleteShader(T),r.deleteShader(E),p=new ds(r,v),S=pv(r,v)}let p;this.getUniforms=function(){return p===void 0&&U(this),p};let S;this.getAttributes=function(){return S===void 0&&U(this),S};let y=t.rendererExtensionParallelShaderCompile===!1;return this.isReady=function(){return y===!1&&(y=r.getProgramParameter(v,rv)),y},this.destroy=function(){i.releaseStatesOfProgram(this),r.deleteProgram(v),this.program=void 0},this.type=t.shaderType,this.name=t.shaderName,this.id=sv++,this.cacheKey=e,this.usedTimes=1,this.program=v,this.vertexShader=T,this.fragmentShader=E,this}var Pv=0,du=class{constructor(){this.shaderCache=new Map,this.materialCache=new Map}update(e,t,i){let r=this._getShaderCacheForMaterial(e);return r.has(t)===!1&&(r.add(t),t.usedTimes++),r.has(i)===!1&&(r.add(i),i.usedTimes++),this}remove(e){let t=this.materialCache.get(e);for(let i of t)i.usedTimes--,i.usedTimes===0&&this.shaderCache.delete(i.code);return this.materialCache.delete(e),this}getVertexShaderStage(e){return this._getShaderStage(e.vertexShader)}getFragmentShaderStage(e){return this._getShaderStage(e.fragmentShader)}dispose(){this.shaderCache.clear(),this.materialCache.clear()}_getShaderCacheForMaterial(e){let t=this.materialCache,i=t.get(e);return i===void 0&&(i=new Set,t.set(e,i)),i}_getShaderStage(e){let t=this.shaderCache,i=t.get(e);return i===void 0&&(i=new fu(e),t.set(e,i)),i}},fu=class{constructor(e){this.id=Pv++,this.code=e,this.usedTimes=0}};function Iv(n){return n===er||n===lo||n===co}function Lv(n,e,t,i,r,s){let o=new Kr,a=new du,l=new Set,c=[],d=new Map,m=i.logarithmicDepthBuffer,u=i.precision,g={MeshDepthMaterial:"depth",MeshDistanceMaterial:"distance",MeshNormalMaterial:"normal",MeshBasicMaterial:"basic",MeshLambertMaterial:"lambert",MeshPhongMaterial:"phong",MeshToonMaterial:"toon",MeshStandardMaterial:"physical",MeshPhysicalMaterial:"physical",MeshMatcapMaterial:"matcap",LineBasicMaterial:"basic",LineDashedMaterial:"dashed",PointsMaterial:"points",ShadowMaterial:"shadow",SpriteMaterial:"sprite"};function _(p){return l.add(p),p===0?"uv":`uv${p}`}function v(p,S,y,R,I,k){let D=R.fog,N=I.geometry,H=p.isMeshStandardMaterial||p.isMeshLambertMaterial||p.isMeshPhongMaterial?R.environment:null,$=p.isMeshStandardMaterial||p.isMeshLambertMaterial&&!p.envMap||p.isMeshPhongMaterial&&!p.envMap,ee=e.get(p.envMap||H,$),J=ee&&ee.mapping===no?ee.image.height:null,ie=g[p.type];p.precision!==null&&(u=i.getMaxPrecision(p.precision),u!==p.precision&&$e("WebGLProgram.getParameters:",p.precision,"not supported, using",u,"instead."));let ce=N.morphAttributes.position||N.morphAttributes.normal||N.morphAttributes.color,we=ce!==void 0?ce.length:0,pe=0;N.morphAttributes.position!==void 0&&(pe=1),N.morphAttributes.normal!==void 0&&(pe=2),N.morphAttributes.color!==void 0&&(pe=3);let Pe,Ve,Oe,oe;if(ie){let _t=mi[ie];Pe=_t.vertexShader,Ve=_t.fragmentShader}else{Pe=p.vertexShader,Ve=p.fragmentShader;let _t=a.getVertexShaderStage(p),ot=a.getFragmentShaderStage(p);a.update(p,_t,ot),Oe=_t.id,oe=ot.id}let le=n.getRenderTarget(),be=n.state.buffers.depth.getReversed(),Ge=I.isInstancedMesh===!0,Me=I.isBatchedMesh===!0,ze=!!p.map,Ut=!!p.matcap,Je=!!ee,st=!!p.aoMap,gt=!!p.lightMap,tt=!!p.bumpMap&&p.wireframe===!1,Tt=!!p.normalMap,Ft=!!p.displacementMap,Wt=!!p.emissiveMap,Pt=!!p.metalnessMap,Ot=!!p.roughnessMap,W=p.anisotropy>0,tn=p.clearcoat>0,ct=p.dispersion>0,F=p.retroreflectivity>0,b=p.iridescence>0,Y=p.sheen>0,te=p.transmission>0,ae=W&&!!p.anisotropyMap,ve=tn&&!!p.clearcoatMap,ye=tn&&!!p.clearcoatNormalMap,C=tn&&!!p.clearcoatRoughnessMap,L=b&&!!p.iridescenceMap,O=b&&!!p.iridescenceThicknessMap,z=Y&&!!p.sheenColorMap,X=Y&&!!p.sheenRoughnessMap,Z=!!p.specularMap,re=!!p.specularColorMap,fe=!!p.specularIntensityMap,me=te&&!!p.transmissionMap,B=te&&!!p.thicknessMap,ue=!!p.gradientMap,se=!!p.alphaMap,ge=p.alphaTest>0,_e=!!p.alphaHash,he=!!p.extensions,Ce=ni;p.toneMapped&&(le===null||le.isXRRenderTarget===!0)&&(Ce=n.toneMapping);let Le={shaderID:ie,shaderType:p.type,shaderName:p.name,vertexShader:Pe,fragmentShader:Ve,defines:p.defines,customVertexShaderID:Oe,customFragmentShaderID:oe,isRawShaderMaterial:p.isRawShaderMaterial===!0,glslVersion:p.glslVersion,precision:u,batching:Me,batchingColor:Me&&I._colorsTexture!==null,instancing:Ge,instancingColor:Ge&&I.instanceColor!==null,instancingMorph:Ge&&I.morphTexture!==null,outputColorSpace:le===null?n.outputColorSpace:le.isXRRenderTarget===!0?le.texture.colorSpace:ut.workingColorSpace,alphaToCoverage:!!p.alphaToCoverage,map:ze,matcap:Ut,envMap:Je,envMapMode:Je&&ee.mapping,envMapCubeUVHeight:J,aoMap:st,lightMap:gt,bumpMap:tt,normalMap:Tt,displacementMap:Ft,emissiveMap:Wt,normalMapObjectSpace:Tt&&p.normalMapType===td,normalMapTangentSpace:Tt&&p.normalMapType===pl,packedNormalMap:Tt&&p.normalMapType===pl&&Iv(p.normalMap.format),metalnessMap:Pt,roughnessMap:Ot,anisotropy:W,anisotropyMap:ae,clearcoat:tn,clearcoatMap:ve,clearcoatNormalMap:ye,clearcoatRoughnessMap:C,dispersion:ct,retroreflection:F,iridescence:b,iridescenceMap:L,iridescenceThicknessMap:O,sheen:Y,sheenColorMap:z,sheenRoughnessMap:X,specularMap:Z,specularColorMap:re,specularIntensityMap:fe,transmission:te,transmissionMap:me,thicknessMap:B,gradientMap:ue,opaque:p.transparent===!1&&p.blending===ss&&p.alphaToCoverage===!1,alphaMap:se,alphaTest:ge,alphaHash:_e,combine:p.combine,mapUv:ze&&_(p.map.channel),aoMapUv:st&&_(p.aoMap.channel),lightMapUv:gt&&_(p.lightMap.channel),bumpMapUv:tt&&_(p.bumpMap.channel),normalMapUv:Tt&&_(p.normalMap.channel),displacementMapUv:Ft&&_(p.displacementMap.channel),emissiveMapUv:Wt&&_(p.emissiveMap.channel),metalnessMapUv:Pt&&_(p.metalnessMap.channel),roughnessMapUv:Ot&&_(p.roughnessMap.channel),anisotropyMapUv:ae&&_(p.anisotropyMap.channel),clearcoatMapUv:ve&&_(p.clearcoatMap.channel),clearcoatNormalMapUv:ye&&_(p.clearcoatNormalMap.channel),clearcoatRoughnessMapUv:C&&_(p.clearcoatRoughnessMap.channel),iridescenceMapUv:L&&_(p.iridescenceMap.channel),iridescenceThicknessMapUv:O&&_(p.iridescenceThicknessMap.channel),sheenColorMapUv:z&&_(p.sheenColorMap.channel),sheenRoughnessMapUv:X&&_(p.sheenRoughnessMap.channel),specularMapUv:Z&&_(p.specularMap.channel),specularColorMapUv:re&&_(p.specularColorMap.channel),specularIntensityMapUv:fe&&_(p.specularIntensityMap.channel),transmissionMapUv:me&&_(p.transmissionMap.channel),thicknessMapUv:B&&_(p.thicknessMap.channel),alphaMapUv:se&&_(p.alphaMap.channel),vertexTangents:!!N.attributes.tangent&&(Tt||W),vertexNormals:!!N.attributes.normal,vertexColors:p.vertexColors,vertexAlphas:p.vertexColors===!0&&!!N.attributes.color&&N.attributes.color.itemSize===4,pointsUvs:I.isPoints===!0&&!!N.attributes.uv&&(ze||se),fog:!!D,useFog:p.fog===!0,fogExp2:!!D&&D.isFogExp2,flatShading:p.wireframe===!1&&(p.flatShading===!0||N.attributes.normal===void 0&&Tt===!1&&(p.isMeshLambertMaterial||p.isMeshPhongMaterial||p.isMeshStandardMaterial||p.isMeshPhysicalMaterial)),sizeAttenuation:p.sizeAttenuation===!0,logarithmicDepthBuffer:m,reversedDepthBuffer:be,skinning:I.isSkinnedMesh===!0,hasPositionAttribute:N.attributes.position!==void 0,morphTargets:N.morphAttributes.position!==void 0,morphNormals:N.morphAttributes.normal!==void 0,morphColors:N.morphAttributes.color!==void 0,morphTargetsCount:we,morphTextureStride:pe,numSunLights:S.sun.length,numDirLights:S.directional.length,numPointLights:S.point.length,numSpotLights:S.spot.length,numSpotLightMaps:S.spotLightMap.length,numRectAreaLights:S.rectArea.length,numHemiLights:S.hemi.length,numSunLightShadows:S.sunShadowMap.length,numDirLightShadows:S.directionalShadowMap.length,numPointLightShadows:S.pointShadowMap.length,numSpotLightShadows:S.spotShadowMap.length,numSpotLightShadowsWithMaps:S.numSpotLightShadowsWithMaps,numLightProbes:S.numLightProbes,numLightProbeGrids:k.length,numClippingPlanes:s.numPlanes,numClipIntersection:s.numIntersection,dithering:p.dithering,shadowMapEnabled:n.shadowMap.enabled&&y.length>0,shadowMapType:n.shadowMap.type,toneMapping:Ce,decodeVideoTexture:ze&&p.map.isVideoTexture===!0&&ut.getTransfer(p.map.colorSpace)===St,decodeVideoTextureEmissive:Wt&&p.emissiveMap.isVideoTexture===!0&&ut.getTransfer(p.emissiveMap.colorSpace)===St,premultipliedAlpha:p.premultipliedAlpha,doubleSided:p.side===Nn,flipSided:p.side===fn,useDepthPacking:p.depthPacking>=0,depthPacking:p.depthPacking||0,index0AttributeName:p.index0AttributeName,extensionClipCullDistance:he&&p.extensions.clipCullDistance===!0&&t.has("WEBGL_clip_cull_distance"),extensionMultiDraw:(he&&p.extensions.multiDraw===!0||Me)&&t.has("WEBGL_multi_draw"),rendererExtensionParallelShaderCompile:t.has("KHR_parallel_shader_compile"),customProgramCacheKey:p.customProgramCacheKey()};return Le.vertexUv1s=l.has(1),Le.vertexUv2s=l.has(2),Le.vertexUv3s=l.has(3),l.clear(),Le}function f(p){let S=[];if(p.shaderID?S.push(p.shaderID):(S.push(p.customVertexShaderID),S.push(p.customFragmentShaderID)),p.defines!==void 0)for(let y in p.defines)S.push(y),S.push(p.defines[y]);return p.isRawShaderMaterial===!1&&(h(S,p),M(S,p),S.push(n.outputColorSpace)),S.push(p.customProgramCacheKey),S.join()}function h(p,S){p.push(S.precision),p.push(S.outputColorSpace),p.push(S.envMapMode),p.push(S.envMapCubeUVHeight),p.push(S.mapUv),p.push(S.alphaMapUv),p.push(S.lightMapUv),p.push(S.aoMapUv),p.push(S.bumpMapUv),p.push(S.normalMapUv),p.push(S.displacementMapUv),p.push(S.emissiveMapUv),p.push(S.metalnessMapUv),p.push(S.roughnessMapUv),p.push(S.anisotropyMapUv),p.push(S.clearcoatMapUv),p.push(S.clearcoatNormalMapUv),p.push(S.clearcoatRoughnessMapUv),p.push(S.iridescenceMapUv),p.push(S.iridescenceThicknessMapUv),p.push(S.sheenColorMapUv),p.push(S.sheenRoughnessMapUv),p.push(S.specularMapUv),p.push(S.specularColorMapUv),p.push(S.specularIntensityMapUv),p.push(S.transmissionMapUv),p.push(S.thicknessMapUv),p.push(S.combine),p.push(S.fogExp2),p.push(S.sizeAttenuation),p.push(S.morphTargetsCount),p.push(S.morphAttributeCount),p.push(S.numSunLights),p.push(S.numDirLights),p.push(S.numPointLights),p.push(S.numSpotLights),p.push(S.numSpotLightMaps),p.push(S.numHemiLights),p.push(S.numRectAreaLights),p.push(S.numSunLightShadows),p.push(S.numDirLightShadows),p.push(S.numPointLightShadows),p.push(S.numSpotLightShadows),p.push(S.numSpotLightShadowsWithMaps),p.push(S.numLightProbes),p.push(S.shadowMapType),p.push(S.toneMapping),p.push(S.numClippingPlanes),p.push(S.numClipIntersection),p.push(S.depthPacking)}function M(p,S){o.disableAll(),S.instancing&&o.enable(0),S.instancingColor&&o.enable(1),S.instancingMorph&&o.enable(2),S.matcap&&o.enable(3),S.envMap&&o.enable(4),S.normalMapObjectSpace&&o.enable(5),S.normalMapTangentSpace&&o.enable(6),S.clearcoat&&o.enable(7),S.iridescence&&o.enable(8),S.alphaTest&&o.enable(9),S.vertexColors&&o.enable(10),S.vertexAlphas&&o.enable(11),S.vertexUv1s&&o.enable(12),S.vertexUv2s&&o.enable(13),S.vertexUv3s&&o.enable(14),S.vertexTangents&&o.enable(15),S.anisotropy&&o.enable(16),S.alphaHash&&o.enable(17),S.batching&&o.enable(18),S.dispersion&&o.enable(19),S.retroreflection&&o.enable(24),S.batchingColor&&o.enable(20),S.gradientMap&&o.enable(21),S.packedNormalMap&&o.enable(22),S.vertexNormals&&o.enable(23),p.push(o.mask),o.disableAll(),S.fog&&o.enable(0),S.useFog&&o.enable(1),S.flatShading&&o.enable(2),S.logarithmicDepthBuffer&&o.enable(3),S.reversedDepthBuffer&&o.enable(4),S.skinning&&o.enable(5),S.morphTargets&&o.enable(6),S.morphNormals&&o.enable(7),S.morphColors&&o.enable(8),S.premultipliedAlpha&&o.enable(9),S.shadowMapEnabled&&o.enable(10),S.doubleSided&&o.enable(11),S.flipSided&&o.enable(12),S.useDepthPacking&&o.enable(13),S.dithering&&o.enable(14),S.transmission&&o.enable(15),S.sheen&&o.enable(16),S.opaque&&o.enable(17),S.pointsUvs&&o.enable(18),S.decodeVideoTexture&&o.enable(19),S.decodeVideoTextureEmissive&&o.enable(20),S.alphaToCoverage&&o.enable(21),S.numLightProbeGrids>0&&o.enable(22),S.hasPositionAttribute&&o.enable(23),p.push(o.mask)}function P(p){let S=g[p.type],y;if(S){let R=mi[S];y=xl.clone(R.uniforms)}else y=p.uniforms;return y}function w(p,S){let y=d.get(S);return y!==void 0?++y.usedTimes:(y=new Cv(n,S,p,r),c.push(y),d.set(S,y)),y}function T(p){if(--p.usedTimes===0){let S=c.indexOf(p);c[S]=c[c.length-1],c.pop(),d.delete(p.cacheKey),p.destroy()}}function E(p){a.remove(p)}function U(){a.dispose()}return{getParameters:v,getProgramCacheKey:f,getUniforms:P,acquireProgram:w,releaseProgram:T,releaseShaderCache:E,programs:c,dispose:U}}function kv(){let n=new WeakMap;function e(o){return n.has(o)}function t(o){let a=n.get(o);return a===void 0&&(a={},n.set(o,a)),a}function i(o){n.delete(o)}function r(o,a,l){n.get(o)[a]=l}function s(){n=new WeakMap}return{has:e,get:t,remove:i,update:r,dispose:s}}function Dv(n,e){return n.groupOrder!==e.groupOrder?n.groupOrder-e.groupOrder:n.renderOrder!==e.renderOrder?n.renderOrder-e.renderOrder:n.material.id!==e.material.id?n.material.id-e.material.id:n.materialVariant!==e.materialVariant?n.materialVariant-e.materialVariant:n.z!==e.z?n.z-e.z:n.id-e.id}function Dd(n,e){return n.groupOrder!==e.groupOrder?n.groupOrder-e.groupOrder:n.renderOrder!==e.renderOrder?n.renderOrder-e.renderOrder:n.z!==e.z?e.z-n.z:n.id-e.id}function Nd(){let n=[],e=0,t=[],i=[],r=[];function s(){e=0,t.length=0,i.length=0,r.length=0}function o(u){let g=0;return u.isInstancedMesh&&(g+=2),u.isSkinnedMesh&&(g+=1),g}function a(u,g,_,v,f,h){let M=n[e];return M===void 0?(M={id:u.id,object:u,geometry:g,material:_,materialVariant:o(u),groupOrder:v,renderOrder:u.renderOrder,z:f,group:h},n[e]=M):(M.id=u.id,M.object=u,M.geometry=g,M.material=_,M.materialVariant=o(u),M.groupOrder=v,M.renderOrder=u.renderOrder,M.z=f,M.group=h),e++,M}function l(u,g,_,v,f,h,M){M.reversedDepth===!0&&(f=-f);let P=a(u,g,_,v,f,h);_.transmission>0?i.push(P):_.transparent===!0?r.push(P):t.push(P)}function c(u,g,_,v,f,h){let M=a(u,g,_,v,f,h);_.transmission>0?i.unshift(M):_.transparent===!0?r.unshift(M):t.unshift(M)}function d(u,g){t.length>1&&t.sort(u||Dv),i.length>1&&i.sort(g||Dd),r.length>1&&r.sort(g||Dd)}function m(){for(let u=e,g=n.length;u<g;u++){let _=n[u];if(_.id===null)break;_.id=null,_.object=null,_.geometry=null,_.material=null,_.group=null}}return{opaque:t,transmissive:i,transparent:r,init:s,push:l,unshift:c,finish:m,sort:d}}function Nv(){let n=new WeakMap;function e(i,r){let s=n.get(i),o;return s===void 0?(o=new Nd,n.set(i,[o])):r>=s.length?(o=new Nd,s.push(o)):o=s[r],o}function t(){n=new WeakMap}return{get:e,dispose:t}}function Uv(){let n={};return{get:function(e){if(n[e.id]!==void 0)return n[e.id];let t;switch(e.type){case"SunLight":case"DirectionalLight":t={direction:new G,color:new Ne};break;case"SpotLight":t={position:new G,direction:new G,color:new Ne,distance:0,coneCos:0,penumbraCos:0,decay:0};break;case"PointLight":t={position:new G,color:new Ne,distance:0,decay:0};break;case"HemisphereLight":t={direction:new G,skyColor:new Ne,groundColor:new Ne};break;case"RectAreaLight":t={color:new Ne,position:new G,halfWidth:new G,halfHeight:new G};break}return n[e.id]=t,t}}}function Fv(){let n={};return{get:function(e){if(n[e.id]!==void 0)return n[e.id];let t;switch(e.type){case"SunLight":case"DirectionalLight":t={shadowIntensity:1,shadowBias:0,shadowNormalBias:0,shadowRadius:1,shadowMapSize:new We};break;case"SpotLight":t={shadowIntensity:1,shadowBias:0,shadowNormalBias:0,shadowRadius:1,shadowMapSize:new We};break;case"PointLight":t={shadowIntensity:1,shadowBias:0,shadowNormalBias:0,shadowRadius:1,shadowMapSize:new We,shadowCameraNear:1,shadowCameraFar:1e3};break}return n[e.id]=t,t}}}var Ov=0;function Bv(n,e){return(e.castShadow?2:0)-(n.castShadow?2:0)+(e.map?1:0)-(n.map?1:0)}function zv(n){let e=new Uv,t=Fv(),i={version:0,hash:{sunLength:-1,directionalLength:-1,pointLength:-1,spotLength:-1,rectAreaLength:-1,hemiLength:-1,numSunShadows:-1,numDirectionalShadows:-1,numPointShadows:-1,numSpotShadows:-1,numSpotMaps:-1,numLightProbes:-1},ambient:[0,0,0],probe:[],sun:[],sunShadow:[],sunShadowMap:[],sunShadowMatrix:[],sunShadowCascade:[],directional:[],directionalShadow:[],directionalShadowMap:[],directionalShadowMatrix:[],spot:[],spotLightMap:[],spotShadow:[],spotShadowMap:[],spotLightMatrix:[],rectArea:[],rectAreaLTC1:null,rectAreaLTC2:null,point:[],pointShadow:[],pointShadowMap:[],pointShadowMatrix:[],hemi:[],numSpotLightShadowsWithMaps:0,numLightProbes:0};for(let c=0;c<9;c++)i.probe.push(new G);let r=new G,s=new wt,o=new wt;function a(c){let d=0,m=0,u=0;for(let I=0;I<9;I++)i.probe[I].set(0,0,0);let g=0,_=0,v=0,f=0,h=0,M=0,P=0,w=0,T=0,E=0,U=0,p=0,S=0,y=0;c.sort(Bv);for(let I=0,k=c.length;I<k;I++){let D=c[I],N=D.color,H=D.intensity,$=D.distance,ee=null;if(D.shadow&&D.shadow.map&&(D.shadow.map.texture.format===er?ee=D.shadow.map.texture:ee=D.shadow.map.depthTexture||D.shadow.map.texture),D.isAmbientLight)d+=N.r*H,m+=N.g*H,u+=N.b*H;else if(D.isLightProbe){for(let J=0;J<9;J++)i.probe[J].addScaledVector(D.sh.coefficients[J],H);y++}else if(D.isSunLight){let J=e.get(D);if(J.color.copy(D.color).multiplyScalar(D.intensity),D.castShadow){let ie=D.shadow,ce=t.get(D);ce.shadowIntensity=ie.intensity,ce.shadowBias=ie.bias,ce.shadowNormalBias=ie.normalBias,ce.shadowRadius=ie.radius,ce.shadowMapSize.copy(ie.mapSize).multiply(ie.getFrameExtents()),i.sunShadow[_]=ce,i.sunShadowMap[_]=ee;let we=ie.getViewportCount();for(let pe=0;pe<we;pe++)i.sunShadowMatrix[v+pe]=ie.getMatrix(pe),i.sunShadowCascade[v+pe]=ie._cascadeData[pe];v+=we,_++}i.sun[g]=J,g++}else if(D.isDirectionalLight){let J=e.get(D);if(J.color.copy(D.color).multiplyScalar(D.intensity),D.castShadow){let ie=D.shadow,ce=t.get(D);ce.shadowIntensity=ie.intensity,ce.shadowBias=ie.bias,ce.shadowNormalBias=ie.normalBias,ce.shadowRadius=ie.radius,ce.shadowMapSize=ie.mapSize,i.directionalShadow[f]=ce,i.directionalShadowMap[f]=ee,i.directionalShadowMatrix[f]=D.shadow.matrix,T++}i.directional[f]=J,f++}else if(D.isSpotLight){let J=e.get(D);J.position.setFromMatrixPosition(D.matrixWorld),J.color.copy(N).multiplyScalar(H),J.distance=$,J.coneCos=Math.cos(D.angle),J.penumbraCos=Math.cos(D.angle*(1-D.penumbra)),J.decay=D.decay,i.spot[M]=J;let ie=D.shadow;if(D.map&&(i.spotLightMap[p]=D.map,p++,ie.updateMatrices(D),D.castShadow&&S++),i.spotLightMatrix[M]=ie.matrix,D.castShadow){let ce=t.get(D);ce.shadowIntensity=ie.intensity,ce.shadowBias=ie.bias,ce.shadowNormalBias=ie.normalBias,ce.shadowRadius=ie.radius,ce.shadowMapSize=ie.mapSize,i.spotShadow[M]=ce,i.spotShadowMap[M]=ee,U++}M++}else if(D.isRectAreaLight){let J=e.get(D);J.color.copy(N).multiplyScalar(H),J.halfWidth.set(D.width*.5,0,0),J.halfHeight.set(0,D.height*.5,0),i.rectArea[P]=J,P++}else if(D.isPointLight){let J=e.get(D);if(J.color.copy(D.color).multiplyScalar(D.intensity),J.distance=D.distance,J.decay=D.decay,D.castShadow){let ie=D.shadow,ce=t.get(D);ce.shadowIntensity=ie.intensity,ce.shadowBias=ie.bias,ce.shadowNormalBias=ie.normalBias,ce.shadowRadius=ie.radius,ce.shadowMapSize=ie.mapSize,ce.shadowCameraNear=ie.camera.near,ce.shadowCameraFar=ie.camera.far,i.pointShadow[h]=ce,i.pointShadowMap[h]=ee,i.pointShadowMatrix[h]=D.shadow.matrix,E++}i.point[h]=J,h++}else if(D.isHemisphereLight){let J=e.get(D);J.skyColor.copy(D.color).multiplyScalar(H),J.groundColor.copy(D.groundColor).multiplyScalar(H),i.hemi[w]=J,w++}}P>0&&(n.has("OES_texture_float_linear")===!0?(i.rectAreaLTC1=Te.LTC_FLOAT_1,i.rectAreaLTC2=Te.LTC_FLOAT_2):(i.rectAreaLTC1=Te.LTC_HALF_1,i.rectAreaLTC2=Te.LTC_HALF_2)),i.ambient[0]=d,i.ambient[1]=m,i.ambient[2]=u;let R=i.hash;(R.sunLength!==g||R.directionalLength!==f||R.pointLength!==h||R.spotLength!==M||R.rectAreaLength!==P||R.hemiLength!==w||R.numSunShadows!==_||R.numDirectionalShadows!==T||R.numPointShadows!==E||R.numSpotShadows!==U||R.numSpotMaps!==p||R.numLightProbes!==y)&&(i.sun.length=g,i.directional.length=f,i.spot.length=M,i.rectArea.length=P,i.point.length=h,i.hemi.length=w,i.sunShadow.length=_,i.sunShadowMap.length=_,i.sunShadowMatrix.length=v,i.sunShadowCascade.length=v,i.directionalShadow.length=T,i.directionalShadowMap.length=T,i.directionalShadowMatrix.length=T,i.pointShadow.length=E,i.pointShadowMap.length=E,i.pointShadowMatrix.length=E,i.spotShadow.length=U,i.spotShadowMap.length=U,i.spotLightMatrix.length=U+p-S,i.spotLightMap.length=p,i.numSpotLightShadowsWithMaps=S,i.numLightProbes=y,R.sunLength=g,R.directionalLength=f,R.pointLength=h,R.spotLength=M,R.rectAreaLength=P,R.hemiLength=w,R.numSunShadows=_,R.numDirectionalShadows=T,R.numPointShadows=E,R.numSpotShadows=U,R.numSpotMaps=p,R.numLightProbes=y,i.version=Ov++)}function l(c,d){let m=0,u=0,g=0,_=0,v=0,f=0,h=d.matrixWorldInverse;for(let M=0,P=c.length;M<P;M++){let w=c[M];if(w.isSunLight){let T=i.sun[m];T.direction.setFromMatrixPosition(w.matrixWorld),T.direction.transformDirection(h),m++}else if(w.isDirectionalLight){let T=i.directional[u];T.direction.setFromMatrixPosition(w.matrixWorld),r.setFromMatrixPosition(w.target.matrixWorld),T.direction.sub(r),T.direction.transformDirection(h),u++}else if(w.isSpotLight){let T=i.spot[_];T.position.setFromMatrixPosition(w.matrixWorld),T.position.applyMatrix4(h),T.direction.setFromMatrixPosition(w.matrixWorld),r.setFromMatrixPosition(w.target.matrixWorld),T.direction.sub(r),T.direction.transformDirection(h),_++}else if(w.isRectAreaLight){let T=i.rectArea[v];T.position.setFromMatrixPosition(w.matrixWorld),T.position.applyMatrix4(h),o.identity(),s.copy(w.matrixWorld),s.premultiply(h),o.extractRotation(s),T.halfWidth.set(w.width*.5,0,0),T.halfHeight.set(0,w.height*.5,0),T.halfWidth.applyMatrix4(o),T.halfHeight.applyMatrix4(o),v++}else if(w.isPointLight){let T=i.point[g];T.position.setFromMatrixPosition(w.matrixWorld),T.position.applyMatrix4(h),g++}else if(w.isHemisphereLight){let T=i.hemi[f];T.direction.setFromMatrixPosition(w.matrixWorld),T.direction.transformDirection(h),f++}}}return{setup:a,setupView:l,state:i}}function Ud(n){let e=new zv(n),t=[],i=[],r=[];function s(u){m.camera=u,t.length=0,i.length=0,r.length=0}function o(u){t.push(u)}function a(u){i.push(u)}function l(u){r.push(u)}function c(){e.setup(t)}function d(u){e.setupView(t,u)}let m={lightsArray:t,shadowsArray:i,lightProbeGridArray:r,camera:null,lights:e,transmissionRenderTarget:{},textureUnits:0};return{init:s,state:m,setupLights:c,setupLightsView:d,pushLight:o,pushShadow:a,pushLightProbeGrid:l}}function Hv(n){let e=new WeakMap;function t(r,s=0){let o=e.get(r),a;return o===void 0?(a=new Ud(n),e.set(r,[a])):s>=o.length?(a=new Ud(n),o.push(a)):a=o[s],a}function i(){e=new WeakMap}return{get:t,dispose:i}}var Gv=`void main() {
	gl_Position = vec4( position, 1.0 );
}`,Vv=`uniform sampler2D shadow_pass;
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
}`,Wv=[new G(1,0,0),new G(-1,0,0),new G(0,1,0),new G(0,-1,0),new G(0,0,1),new G(0,0,-1)],Xv=[new G(0,-1,0),new G(0,-1,0),new G(0,0,1),new G(0,0,-1),new G(0,-1,0),new G(0,-1,0)],Fd=new wt,ho=new G,ou=new G;function qv(n,e,t){let i=new jr,r=new We,s=new We,o=new Bt,a=new pa,l=new ma,c={},d=t.maxTextureSize,m={[Ki]:fn,[fn]:Ki,[Nn]:Nn},u=new Tn({defines:{VSM_SAMPLES:8},uniforms:{shadow_pass:{value:null},resolution:{value:new We},radius:{value:4}},vertexShader:Gv,fragmentShader:Vv}),g=u.clone();g.defines.HORIZONTAL_PASS=1;let _=new $t;_.setAttribute("position",new Pn(new Float32Array([-1,-1,.5,3,-1,.5,-1,3,.5]),3));let v=new rt(_,u),f=this;this.enabled=!1,this.autoUpdate=!0,this.needsUpdate=!1,this.type=mr;let h=this.type;this.render=function(E,U,p){if(f.enabled===!1||f.autoUpdate===!1&&f.needsUpdate===!1||E.length===0)return;this.type===Lh&&($e("WebGLShadowMap: PCFSoftShadowMap has been removed. Using PCFShadowMap instead."),this.type=mr);let S=n.getRenderTarget(),y=n.getActiveCubeFace(),R=n.getActiveMipmapLevel(),I=n.state;I.setBlending(fi),I.buffers.depth.getReversed()===!0?I.buffers.color.setClear(0,0,0,0):I.buffers.color.setClear(1,1,1,1),I.buffers.depth.setTest(!0),I.setScissorTest(!1);let k=h!==this.type;k&&U.traverse(function(D){D.material&&(Array.isArray(D.material)?D.material.forEach(N=>N.needsUpdate=!0):D.material.needsUpdate=!0)});for(let D=0,N=E.length;D<N;D++){let H=E[D],$=H.shadow;if($===void 0){$e("WebGLShadowMap:",H,"has no shadow.");continue}if($.autoUpdate===!1&&$.needsUpdate===!1)continue;r.copy($.mapSize);let ee=$.getFrameExtents();r.multiply(ee),s.copy($.mapSize),(r.x>d||r.y>d)&&(r.x>d&&(s.x=Math.floor(d/ee.x),r.x=s.x*ee.x,$.mapSize.x=s.x),r.y>d&&(s.y=Math.floor(d/ee.y),r.y=s.y*ee.y,$.mapSize.y=s.y));let J=n.state.buffers.depth.getReversed();if($.camera._reversedDepth=J,$.map===null||k===!0){if($.map!==null&&($.map.depthTexture!==null&&($.map.depthTexture.dispose(),$.map.depthTexture=null),$.map.dispose()),this.type===rs){if(H.isPointLight){$e("WebGLShadowMap: VSM shadow maps are not supported for PointLights. Use PCF or BasicShadowMap instead.");continue}$.map=new kn(r.x,r.y,{format:er,type:ri,minFilter:xn,magFilter:xn,generateMipmaps:!1}),$.map.texture.name=H.name+".shadowMap",$.map.depthTexture=new Gi(r.x,r.y,$n),$.map.depthTexture.name=H.name+".shadowMapDepth",$.map.depthTexture.format=hi,$.map.depthTexture.compareFunction=null,$.map.depthTexture.minFilter=Qt,$.map.depthTexture.magFilter=Qt}else H.isPointLight?($.map=new bl(r.x),$.map.depthTexture=new da(r.x,ii)):($.map=new kn(r.x,r.y),$.map.depthTexture=new Gi(r.x,r.y,ii)),$.map.depthTexture.name=H.name+".shadowMap",$.map.depthTexture.format=hi,this.type===mr?($.map.depthTexture.compareFunction=J?gl:ml,$.map.depthTexture.minFilter=xn,$.map.depthTexture.magFilter=xn):($.map.depthTexture.compareFunction=null,$.map.depthTexture.minFilter=Qt,$.map.depthTexture.magFilter=Qt);$.camera.updateProjectionMatrix()}$.map.isWebGLCubeRenderTarget!==!0&&($.map.width!==r.x||$.map.height!==r.y)&&$.map.setSize(r.x,r.y);let ie=$.map.isWebGLCubeRenderTarget?6:$.getViewportCount();H.isPointLight!==!0&&$.updateMatrices(H,p);for(let ce=0;ce<ie;ce++){let we=$.getCamera(ce);if(H.isPointLight){let pe=$.camera,Pe=$.matrix,Ve=H.distance||pe.far;Ve!==pe.far&&(pe.far=Ve,pe.updateProjectionMatrix()),ho.setFromMatrixPosition(H.matrixWorld),pe.position.copy(ho),ou.copy(pe.position),ou.add(Wv[ce]),pe.up.copy(Xv[ce]),pe.lookAt(ou),pe.updateMatrixWorld(),Pe.makeTranslation(-ho.x,-ho.y,-ho.z),Fd.multiplyMatrices(pe.projectionMatrix,pe.matrixWorldInverse),$._frustum.setFromProjectionMatrix(Fd,pe.coordinateSystem,pe.reversedDepth)}if($.map.isWebGLCubeRenderTarget)n.setRenderTarget($.map,ce),n.clear();else{ce===0&&(n.setRenderTarget($.map),n.clear());let pe=$.getViewport(ce);o.set(s.x*pe.x,s.y*pe.y,s.x*pe.z,s.y*pe.w),I.viewport(o)}i=$.getFrustum(ce),w(U,p,we,H,this.type)}$.isPointLightShadow!==!0&&this.type===rs&&M($,p),$.needsUpdate=!1}h=this.type,f.needsUpdate=!1,n.setRenderTarget(S,y,R)};function M(E,U){let p=e.update(v);u.defines.VSM_SAMPLES!==E.blurSamples&&(u.defines.VSM_SAMPLES=E.blurSamples,g.defines.VSM_SAMPLES=E.blurSamples,u.needsUpdate=!0,g.needsUpdate=!0),E.mapPass===null?E.mapPass=new kn(r.x,r.y,{format:er,type:ri}):(E.mapPass.width!==E.map.width||E.mapPass.height!==E.map.height)&&E.mapPass.setSize(E.map.width,E.map.height),u.uniforms.shadow_pass.value=E.map.depthTexture,u.uniforms.resolution.value.set(E.map.width,E.map.height),u.uniforms.radius.value=E.radius,n.setRenderTarget(E.mapPass),n.clear(),n.renderBufferDirect(U,null,p,u,v,null),g.uniforms.shadow_pass.value=E.mapPass.texture,g.uniforms.resolution.value.set(E.map.width,E.map.height),g.uniforms.radius.value=E.radius,n.setRenderTarget(E.map),n.clear(),n.renderBufferDirect(U,null,p,g,v,null)}function P(E,U,p,S){let y=null,R=p.isPointLight===!0?E.customDistanceMaterial:E.customDepthMaterial;if(R!==void 0)y=R;else if(y=p.isPointLight===!0?l:a,n.localClippingEnabled&&U.clipShadows===!0&&Array.isArray(U.clippingPlanes)&&U.clippingPlanes.length!==0||U.displacementMap&&U.displacementScale!==0||U.alphaMap&&U.alphaTest>0||U.map&&U.alphaTest>0||U.alphaToCoverage===!0){let I=y.uuid,k=U.uuid,D=c[I];D===void 0&&(D={},c[I]=D);let N=D[k];N===void 0&&(N=y.clone(),D[k]=N,U.addEventListener("dispose",T)),y=N}if(y.visible=U.visible,y.wireframe=U.wireframe,S===rs?y.side=U.shadowSide!==null?U.shadowSide:U.side:y.side=U.shadowSide!==null?U.shadowSide:m[U.side],y.alphaMap=U.alphaMap,y.alphaTest=U.alphaToCoverage===!0?.5:U.alphaTest,y.map=U.map,y.clipShadows=U.clipShadows,y.clippingPlanes=U.clippingPlanes,y.clipIntersection=U.clipIntersection,y.displacementMap=U.displacementMap,y.displacementScale=U.displacementScale,y.displacementBias=U.displacementBias,y.wireframeLinewidth=U.wireframeLinewidth,y.linewidth=U.linewidth,p.isPointLight===!0&&y.isMeshDistanceMaterial===!0){let I=n.properties.get(y);I.light=p}return y}function w(E,U,p,S,y){if(E.visible===!1)return;if(E.layers.test(U.layers)&&(E.isMesh||E.isLine||E.isPoints)&&(E.castShadow||E.receiveShadow&&y===rs)&&(!E.frustumCulled||E.intersectsFrustum(i))){E.modelViewMatrix.multiplyMatrices(p.matrixWorldInverse,E.matrixWorld);let k=e.update(E),D=E.material;if(Array.isArray(D)){let N=k.groups;for(let H=0,$=N.length;H<$;H++){let ee=N[H],J=D[ee.materialIndex];if(J&&J.visible){let ie=P(E,J,S,y);E.onBeforeShadow(n,E,U,p,k,ie,ee),n.renderBufferDirect(p,null,k,ie,E,ee),E.onAfterShadow(n,E,U,p,k,ie,ee)}}}else if(D.visible){let N=P(E,D,S,y);E.onBeforeShadow(n,E,U,p,k,N,null),n.renderBufferDirect(p,null,k,N,E,null),E.onAfterShadow(n,E,U,p,k,N,null)}}let I=E.children;for(let k=0,D=I.length;k<D;k++)w(I[k],U,p,S,y)}function T(E){E.target.removeEventListener("dispose",T);for(let p in c){let S=c[p],y=E.target.uuid;y in S&&(S[y].dispose(),delete S[y])}}}function $v(n,e){function t(){let B=!1,ue=new Bt,se=null,ge=new Bt(0,0,0,0);return{setMask:function(_e){se!==_e&&!B&&(n.colorMask(_e,_e,_e,_e),se=_e)},setLocked:function(_e){B=_e},setClear:function(_e,he,Ce,Le,_t){_t===!0&&(_e*=Le,he*=Le,Ce*=Le),ue.set(_e,he,Ce,Le),ge.equals(ue)===!1&&(n.clearColor(_e,he,Ce,Le),ge.copy(ue))},reset:function(){B=!1,se=null,ge.set(-1,0,0,0)}}}function i(){let B=!1,ue=!1,se=null,ge=null,_e=null;return{setReversed:function(he){if(ue!==he){let Ce=e.get("EXT_clip_control");he?Ce.clipControlEXT(Ce.LOWER_LEFT_EXT,Ce.ZERO_TO_ONE_EXT):Ce.clipControlEXT(Ce.LOWER_LEFT_EXT,Ce.NEGATIVE_ONE_TO_ONE_EXT),ue=he;let Le=_e;_e=null,this.setClear(Le)}},getReversed:function(){return ue},setTest:function(he){he?le(n.DEPTH_TEST):be(n.DEPTH_TEST)},setMask:function(he){se!==he&&!B&&(n.depthMask(he),se=he)},setFunc:function(he){if(ue&&(he=fd[he]),ge!==he){switch(he){case ea:n.depthFunc(n.NEVER);break;case ta:n.depthFunc(n.ALWAYS);break;case na:n.depthFunc(n.LESS);break;case Xr:n.depthFunc(n.LEQUAL);break;case ia:n.depthFunc(n.EQUAL);break;case ra:n.depthFunc(n.GEQUAL);break;case sa:n.depthFunc(n.GREATER);break;case oa:n.depthFunc(n.NOTEQUAL);break;default:n.depthFunc(n.LEQUAL)}ge=he}},setLocked:function(he){B=he},setClear:function(he){_e!==he&&(_e=he,ue&&(he=1-he),n.clearDepth(he))},reset:function(){B=!1,se=null,ge=null,_e=null,ue=!1}}}function r(){let B=!1,ue=null,se=null,ge=null,_e=null,he=null,Ce=null,Le=null,_t=null;return{setTest:function(ot){B||(ot?le(n.STENCIL_TEST):be(n.STENCIL_TEST))},setMask:function(ot){ue!==ot&&!B&&(n.stencilMask(ot),ue=ot)},setFunc:function(ot,Rt,on){(se!==ot||ge!==Rt||_e!==on)&&(n.stencilFunc(ot,Rt,on),se=ot,ge=Rt,_e=on)},setOp:function(ot,Rt,on){(he!==ot||Ce!==Rt||Le!==on)&&(n.stencilOp(ot,Rt,on),he=ot,Ce=Rt,Le=on)},setLocked:function(ot){B=ot},setClear:function(ot){_t!==ot&&(n.clearStencil(ot),_t=ot)},reset:function(){B=!1,ue=null,se=null,ge=null,_e=null,he=null,Ce=null,Le=null,_t=null}}}let s=new t,o=new i,a=new r,l=new WeakMap,c=new WeakMap,d={},m={},u={},g=new WeakMap,_=[],v=null,f=!1,h=null,M=null,P=null,w=null,T=null,E=null,U=null,p=new Ne(0,0,0),S=0,y=!1,R=null,I=null,k=null,D=null,N=null,H=n.getParameter(n.MAX_COMBINED_TEXTURE_IMAGE_UNITS),$=!1,ee=0,J=n.getParameter(n.VERSION);J.indexOf("WebGL")!==-1?(ee=parseFloat(/^WebGL (\d)/.exec(J)[1]),$=ee>=1):J.indexOf("OpenGL ES")!==-1&&(ee=parseFloat(/^OpenGL ES (\d)/.exec(J)[1]),$=ee>=2);let ie=null,ce={},we=n.getParameter(n.SCISSOR_BOX),pe=n.getParameter(n.VIEWPORT),Pe=new Bt().fromArray(we),Ve=new Bt().fromArray(pe);function Oe(B,ue,se,ge){let _e=new Uint8Array(4),he=n.createTexture();n.bindTexture(B,he),n.texParameteri(B,n.TEXTURE_MIN_FILTER,n.NEAREST),n.texParameteri(B,n.TEXTURE_MAG_FILTER,n.NEAREST);for(let Ce=0;Ce<se;Ce++)B===n.TEXTURE_3D||B===n.TEXTURE_2D_ARRAY?n.texImage3D(ue,0,n.RGBA,1,1,ge,0,n.RGBA,n.UNSIGNED_BYTE,_e):n.texImage2D(ue+Ce,0,n.RGBA,1,1,0,n.RGBA,n.UNSIGNED_BYTE,_e);return he}let oe={};oe[n.TEXTURE_2D]=Oe(n.TEXTURE_2D,n.TEXTURE_2D,1),oe[n.TEXTURE_CUBE_MAP]=Oe(n.TEXTURE_CUBE_MAP,n.TEXTURE_CUBE_MAP_POSITIVE_X,6),oe[n.TEXTURE_2D_ARRAY]=Oe(n.TEXTURE_2D_ARRAY,n.TEXTURE_2D_ARRAY,1,1),oe[n.TEXTURE_3D]=Oe(n.TEXTURE_3D,n.TEXTURE_3D,1,1),s.setClear(0,0,0,1),o.setClear(1),a.setClear(0),le(n.DEPTH_TEST),o.setFunc(Xr),tt(!1),Tt(Ac),le(n.CULL_FACE),st(fi);function le(B){d[B]!==!0&&(n.enable(B),d[B]=!0)}function be(B){d[B]!==!1&&(n.disable(B),d[B]=!1)}function Ge(B,ue){return u[B]!==ue?(n.bindFramebuffer(B,ue),u[B]=ue,B===n.DRAW_FRAMEBUFFER&&(u[n.FRAMEBUFFER]=ue),B===n.FRAMEBUFFER&&(u[n.DRAW_FRAMEBUFFER]=ue),!0):!1}function Me(B,ue){let se=_,ge=!1;if(B){se=g.get(ue),se===void 0&&(se=[],g.set(ue,se));let _e=B.textures;if(se.length!==_e.length||se[0]!==n.COLOR_ATTACHMENT0){for(let he=0,Ce=_e.length;he<Ce;he++)se[he]=n.COLOR_ATTACHMENT0+he;se.length=_e.length,ge=!0}}else se[0]!==n.BACK&&(se[0]=n.BACK,ge=!0);ge&&n.drawBuffers(se)}function ze(B){return v!==B?(n.useProgram(B),v=B,!0):!1}let Ut={[gr]:n.FUNC_ADD,[Dh]:n.FUNC_SUBTRACT,[Nh]:n.FUNC_REVERSE_SUBTRACT};Ut[Uh]=n.MIN,Ut[Fh]=n.MAX;let Je={[Oh]:n.ZERO,[Bh]:n.ONE,[zh]:n.SRC_COLOR,[Pc]:n.SRC_ALPHA,[qh]:n.SRC_ALPHA_SATURATE,[Wh]:n.DST_COLOR,[Gh]:n.DST_ALPHA,[Hh]:n.ONE_MINUS_SRC_COLOR,[Ic]:n.ONE_MINUS_SRC_ALPHA,[Xh]:n.ONE_MINUS_DST_COLOR,[Vh]:n.ONE_MINUS_DST_ALPHA,[$h]:n.CONSTANT_COLOR,[Yh]:n.ONE_MINUS_CONSTANT_COLOR,[Zh]:n.CONSTANT_ALPHA,[Kh]:n.ONE_MINUS_CONSTANT_ALPHA};function st(B,ue,se,ge,_e,he,Ce,Le,_t,ot){if(B===fi){f===!0&&(be(n.BLEND),f=!1);return}if(f===!1&&(le(n.BLEND),f=!0),B!==kh){if(B!==h||ot!==y){if((M!==gr||T!==gr)&&(n.blendEquation(n.FUNC_ADD),M=gr,T=gr),ot)switch(B){case ss:n.blendFuncSeparate(n.ONE,n.ONE_MINUS_SRC_ALPHA,n.ONE,n.ONE_MINUS_SRC_ALPHA);break;case os:n.blendFunc(n.ONE,n.ONE);break;case Rc:n.blendFuncSeparate(n.ZERO,n.ONE_MINUS_SRC_COLOR,n.ZERO,n.ONE);break;case Cc:n.blendFuncSeparate(n.DST_COLOR,n.ONE_MINUS_SRC_ALPHA,n.ZERO,n.ONE);break;default:Ze("WebGLState: Invalid blending: ",B);break}else switch(B){case ss:n.blendFuncSeparate(n.SRC_ALPHA,n.ONE_MINUS_SRC_ALPHA,n.ONE,n.ONE_MINUS_SRC_ALPHA);break;case os:n.blendFuncSeparate(n.SRC_ALPHA,n.ONE,n.ONE,n.ONE);break;case Rc:Ze("WebGLState: SubtractiveBlending requires material.premultipliedAlpha = true");break;case Cc:Ze("WebGLState: MultiplyBlending requires material.premultipliedAlpha = true");break;default:Ze("WebGLState: Invalid blending: ",B);break}P=null,w=null,E=null,U=null,p.set(0,0,0),S=0,h=B,y=ot}return}_e=_e||ue,he=he||se,Ce=Ce||ge,(ue!==M||_e!==T)&&(n.blendEquationSeparate(Ut[ue],Ut[_e]),M=ue,T=_e),(se!==P||ge!==w||he!==E||Ce!==U)&&(n.blendFuncSeparate(Je[se],Je[ge],Je[he],Je[Ce]),P=se,w=ge,E=he,U=Ce),(Le.equals(p)===!1||_t!==S)&&(n.blendColor(Le.r,Le.g,Le.b,_t),p.copy(Le),S=_t),h=B,y=!1}function gt(B,ue){B.side===Nn?be(n.CULL_FACE):le(n.CULL_FACE);let se=B.side===fn;ue&&(se=!se),tt(se),B.blending===ss&&B.transparent===!1?st(fi):st(B.blending,B.blendEquation,B.blendSrc,B.blendDst,B.blendEquationAlpha,B.blendSrcAlpha,B.blendDstAlpha,B.blendColor,B.blendAlpha,B.premultipliedAlpha),o.setFunc(B.depthFunc),o.setTest(B.depthTest),o.setMask(B.depthWrite),s.setMask(B.colorWrite);let ge=B.stencilWrite;a.setTest(ge),ge&&(a.setMask(B.stencilWriteMask),a.setFunc(B.stencilFunc,B.stencilRef,B.stencilFuncMask),a.setOp(B.stencilFail,B.stencilZFail,B.stencilZPass)),Wt(B.polygonOffset,B.polygonOffsetFactor,B.polygonOffsetUnits),B.alphaToCoverage===!0?le(n.SAMPLE_ALPHA_TO_COVERAGE):be(n.SAMPLE_ALPHA_TO_COVERAGE)}function tt(B){R!==B&&(B?n.frontFace(n.CW):n.frontFace(n.CCW),R=B)}function Tt(B){B!==Ph?(le(n.CULL_FACE),B!==I&&(B===Ac?n.cullFace(n.BACK):B===Ih?n.cullFace(n.FRONT):n.cullFace(n.FRONT_AND_BACK))):be(n.CULL_FACE),I=B}function Ft(B){B!==k&&($&&n.lineWidth(B),k=B)}function Wt(B,ue,se){B?(le(n.POLYGON_OFFSET_FILL),(D!==ue||N!==se)&&(D=ue,N=se,o.getReversed()&&(ue=-ue),n.polygonOffset(ue,se))):be(n.POLYGON_OFFSET_FILL)}function Pt(B){B?le(n.SCISSOR_TEST):be(n.SCISSOR_TEST)}function Ot(B){B===void 0&&(B=n.TEXTURE0+H-1),ie!==B&&(n.activeTexture(B),ie=B)}function W(B,ue,se){se===void 0&&(ie===null?se=n.TEXTURE0+H-1:se=ie);let ge=ce[se];ge===void 0&&(ge={type:void 0,texture:void 0},ce[se]=ge),(ge.type!==B||ge.texture!==ue)&&(ie!==se&&(n.activeTexture(se),ie=se),n.bindTexture(B,ue||oe[B]),ge.type=B,ge.texture=ue)}function tn(){let B=ce[ie];B!==void 0&&B.type!==void 0&&(n.bindTexture(B.type,null),B.type=void 0,B.texture=void 0)}function ct(){try{n.compressedTexImage2D(...arguments)}catch(B){Ze("WebGLState:",B)}}function F(){try{n.compressedTexImage3D(...arguments)}catch(B){Ze("WebGLState:",B)}}function b(){try{n.texSubImage2D(...arguments)}catch(B){Ze("WebGLState:",B)}}function Y(){try{n.texSubImage3D(...arguments)}catch(B){Ze("WebGLState:",B)}}function te(){try{n.compressedTexSubImage2D(...arguments)}catch(B){Ze("WebGLState:",B)}}function ae(){try{n.compressedTexSubImage3D(...arguments)}catch(B){Ze("WebGLState:",B)}}function ve(){try{n.texStorage2D(...arguments)}catch(B){Ze("WebGLState:",B)}}function ye(){try{n.texStorage3D(...arguments)}catch(B){Ze("WebGLState:",B)}}function C(){try{n.texImage2D(...arguments)}catch(B){Ze("WebGLState:",B)}}function L(){try{n.texImage3D(...arguments)}catch(B){Ze("WebGLState:",B)}}function O(B){return m[B]!==void 0?m[B]:n.getParameter(B)}function z(B,ue){m[B]!==ue&&(n.pixelStorei(B,ue),m[B]=ue)}function X(B){Pe.equals(B)===!1&&(n.scissor(B.x,B.y,B.z,B.w),Pe.copy(B))}function Z(B){Ve.equals(B)===!1&&(n.viewport(B.x,B.y,B.z,B.w),Ve.copy(B))}function re(B,ue){let se=c.get(ue);se===void 0&&(se=new WeakMap,c.set(ue,se));let ge=se.get(B);ge===void 0&&(ge=n.getUniformBlockIndex(ue,B.name),se.set(B,ge))}function fe(B,ue){let ge=c.get(ue).get(B);l.get(ue)!==ge&&(n.uniformBlockBinding(ue,ge,B.__bindingPointIndex),l.set(ue,ge))}function me(){n.disable(n.BLEND),n.disable(n.CULL_FACE),n.disable(n.DEPTH_TEST),n.disable(n.POLYGON_OFFSET_FILL),n.disable(n.SCISSOR_TEST),n.disable(n.STENCIL_TEST),n.disable(n.SAMPLE_ALPHA_TO_COVERAGE),n.blendEquation(n.FUNC_ADD),n.blendFunc(n.ONE,n.ZERO),n.blendFuncSeparate(n.ONE,n.ZERO,n.ONE,n.ZERO),n.blendColor(0,0,0,0),n.colorMask(!0,!0,!0,!0),n.clearColor(0,0,0,0),n.depthMask(!0),n.depthFunc(n.LESS),o.setReversed(!1),n.clearDepth(1),n.stencilMask(4294967295),n.stencilFunc(n.ALWAYS,0,4294967295),n.stencilOp(n.KEEP,n.KEEP,n.KEEP),n.clearStencil(0),n.cullFace(n.BACK),n.frontFace(n.CCW),n.polygonOffset(0,0),n.activeTexture(n.TEXTURE0),n.bindFramebuffer(n.FRAMEBUFFER,null),n.bindFramebuffer(n.DRAW_FRAMEBUFFER,null),n.bindFramebuffer(n.READ_FRAMEBUFFER,null),n.useProgram(null),n.lineWidth(1),n.scissor(0,0,n.canvas.width,n.canvas.height),n.viewport(0,0,n.canvas.width,n.canvas.height),n.pixelStorei(n.PACK_ALIGNMENT,4),n.pixelStorei(n.UNPACK_ALIGNMENT,4),n.pixelStorei(n.UNPACK_FLIP_Y_WEBGL,!1),n.pixelStorei(n.UNPACK_PREMULTIPLY_ALPHA_WEBGL,!1),n.pixelStorei(n.UNPACK_COLORSPACE_CONVERSION_WEBGL,n.BROWSER_DEFAULT_WEBGL),n.pixelStorei(n.PACK_ROW_LENGTH,0),n.pixelStorei(n.PACK_SKIP_PIXELS,0),n.pixelStorei(n.PACK_SKIP_ROWS,0),n.pixelStorei(n.UNPACK_ROW_LENGTH,0),n.pixelStorei(n.UNPACK_IMAGE_HEIGHT,0),n.pixelStorei(n.UNPACK_SKIP_PIXELS,0),n.pixelStorei(n.UNPACK_SKIP_ROWS,0),n.pixelStorei(n.UNPACK_SKIP_IMAGES,0),d={},m={},ie=null,ce={},u={},g=new WeakMap,_=[],v=null,f=!1,h=null,M=null,P=null,w=null,T=null,E=null,U=null,p=new Ne(0,0,0),S=0,y=!1,R=null,I=null,k=null,D=null,N=null,Pe.set(0,0,n.canvas.width,n.canvas.height),Ve.set(0,0,n.canvas.width,n.canvas.height),s.reset(),o.reset(),a.reset()}return{buffers:{color:s,depth:o,stencil:a},enable:le,disable:be,bindFramebuffer:Ge,drawBuffers:Me,useProgram:ze,setBlending:st,setMaterial:gt,setFlipSided:tt,setCullFace:Tt,setLineWidth:Ft,setPolygonOffset:Wt,setScissorTest:Pt,activeTexture:Ot,bindTexture:W,unbindTexture:tn,compressedTexImage2D:ct,compressedTexImage3D:F,texImage2D:C,texImage3D:L,pixelStorei:z,getParameter:O,updateUBOMapping:re,uniformBlockBinding:fe,texStorage2D:ve,texStorage3D:ye,texSubImage2D:b,texSubImage3D:Y,compressedTexSubImage2D:te,compressedTexSubImage3D:ae,scissor:X,viewport:Z,reset:me}}function Yv(n,e,t,i,r,s,o){let a=e.has("WEBGL_multisampled_render_to_texture")?e.get("WEBGL_multisampled_render_to_texture"):null,l=typeof navigator>"u"?!1:/OculusBrowser/g.test(navigator.userAgent),c=new We,d=new WeakMap,m=new Set,u,g=new WeakMap,_=!1;try{_=typeof OffscreenCanvas<"u"&&new OffscreenCanvas(1,1).getContext("2d")!==null}catch{}function v(F,b){return _?new OffscreenCanvas(F,b):Ls("canvas")}function f(F,b,Y){let te=1,ae=ct(F);if((ae.width>Y||ae.height>Y)&&(te=Y/Math.max(ae.width,ae.height)),te<1)if(typeof HTMLImageElement<"u"&&F instanceof HTMLImageElement||typeof HTMLCanvasElement<"u"&&F instanceof HTMLCanvasElement||typeof ImageBitmap<"u"&&F instanceof ImageBitmap||typeof VideoFrame<"u"&&F instanceof VideoFrame){let ve=Math.floor(te*ae.width),ye=Math.floor(te*ae.height);u===void 0&&(u=v(ve,ye));let C=b?v(ve,ye):u;return C.width=ve,C.height=ye,C.getContext("2d").drawImage(F,0,0,ve,ye),$e("WebGLRenderer: Texture has been resized from ("+ae.width+"x"+ae.height+") to ("+ve+"x"+ye+")."),C}else return"data"in F&&$e("WebGLRenderer: Image in DataTexture is too big ("+ae.width+"x"+ae.height+")."),F;return F}function h(F){return F.generateMipmaps}function M(F){n.generateMipmap(F)}function P(F){return F.isWebGLCubeRenderTarget?n.TEXTURE_CUBE_MAP:F.isWebGL3DRenderTarget?n.TEXTURE_3D:F.isWebGLArrayRenderTarget||F.isCompressedArrayTexture?n.TEXTURE_2D_ARRAY:n.TEXTURE_2D}function w(F,b,Y,te,ae,ve=!1){if(F!==null){if(n[F]!==void 0)return n[F];$e("WebGLRenderer: Attempt to use non-existing WebGL internal format '"+F+"'")}let ye;te&&(ye=e.get("EXT_texture_norm16"),ye||$e("WebGLRenderer: Unable to use normalized textures without EXT_texture_norm16 extension"));let C=b;if(b===n.RED&&(Y===n.FLOAT&&(C=n.R32F),Y===n.HALF_FLOAT&&(C=n.R16F),Y===n.UNSIGNED_BYTE&&(C=n.R8),Y===n.UNSIGNED_SHORT&&ye&&(C=ye.R16_EXT),Y===n.SHORT&&ye&&(C=ye.R16_SNORM_EXT)),b===n.RED_INTEGER&&(Y===n.UNSIGNED_BYTE&&(C=n.R8UI),Y===n.UNSIGNED_SHORT&&(C=n.R16UI),Y===n.UNSIGNED_INT&&(C=n.R32UI),Y===n.BYTE&&(C=n.R8I),Y===n.SHORT&&(C=n.R16I),Y===n.INT&&(C=n.R32I)),b===n.RG&&(Y===n.FLOAT&&(C=n.RG32F),Y===n.HALF_FLOAT&&(C=n.RG16F),Y===n.UNSIGNED_BYTE&&(C=n.RG8),Y===n.UNSIGNED_SHORT&&ye&&(C=ye.RG16_EXT),Y===n.SHORT&&ye&&(C=ye.RG16_SNORM_EXT)),b===n.RG_INTEGER&&(Y===n.UNSIGNED_BYTE&&(C=n.RG8UI),Y===n.UNSIGNED_SHORT&&(C=n.RG16UI),Y===n.UNSIGNED_INT&&(C=n.RG32UI),Y===n.BYTE&&(C=n.RG8I),Y===n.SHORT&&(C=n.RG16I),Y===n.INT&&(C=n.RG32I)),b===n.RGB_INTEGER&&(Y===n.UNSIGNED_BYTE&&(C=n.RGB8UI),Y===n.UNSIGNED_SHORT&&(C=n.RGB16UI),Y===n.UNSIGNED_INT&&(C=n.RGB32UI),Y===n.BYTE&&(C=n.RGB8I),Y===n.SHORT&&(C=n.RGB16I),Y===n.INT&&(C=n.RGB32I)),b===n.RGBA_INTEGER&&(Y===n.UNSIGNED_BYTE&&(C=n.RGBA8UI),Y===n.UNSIGNED_SHORT&&(C=n.RGBA16UI),Y===n.UNSIGNED_INT&&(C=n.RGBA32UI),Y===n.BYTE&&(C=n.RGBA8I),Y===n.SHORT&&(C=n.RGBA16I),Y===n.INT&&(C=n.RGBA32I)),b===n.RGB&&(Y===n.UNSIGNED_SHORT&&ye&&(C=ye.RGB16_EXT),Y===n.SHORT&&ye&&(C=ye.RGB16_SNORM_EXT),Y===n.UNSIGNED_INT_5_9_9_9_REV&&(C=n.RGB9_E5),Y===n.UNSIGNED_INT_10F_11F_11F_REV&&(C=n.R11F_G11F_B10F)),b===n.RGBA){let L=ve?Is:ut.getTransfer(ae);Y===n.FLOAT&&(C=n.RGBA32F),Y===n.HALF_FLOAT&&(C=n.RGBA16F),Y===n.UNSIGNED_BYTE&&(C=L===St?n.SRGB8_ALPHA8:n.RGBA8),Y===n.UNSIGNED_SHORT&&ye&&(C=ye.RGBA16_EXT),Y===n.SHORT&&ye&&(C=ye.RGBA16_SNORM_EXT),Y===n.UNSIGNED_SHORT_4_4_4_4&&(C=n.RGBA4),Y===n.UNSIGNED_SHORT_5_5_5_1&&(C=n.RGB5_A1)}return(C===n.R16F||C===n.R32F||C===n.RG16F||C===n.RG32F||C===n.RGBA16F||C===n.RGBA32F)&&e.get("EXT_color_buffer_float"),C}function T(F,b){let Y;return F?b===null||b===ii||b===ls?Y=n.DEPTH24_STENCIL8:b===$n?Y=n.DEPTH32F_STENCIL8:b===as&&(Y=n.DEPTH24_STENCIL8,$e("DepthTexture: 16 bit depth attachment is not supported with stencil. Using 24-bit attachment.")):b===null||b===ii||b===ls?Y=n.DEPTH_COMPONENT24:b===$n?Y=n.DEPTH_COMPONENT32F:b===as&&(Y=n.DEPTH_COMPONENT16),Y}function E(F,b){return h(F)===!0||F.isFramebufferTexture&&F.minFilter!==Qt&&F.minFilter!==xn?Math.log2(Math.max(b.width,b.height))+1:F.mipmaps!==void 0&&F.mipmaps.length>0?F.mipmaps.length:F.isCompressedTexture&&Array.isArray(F.image)?b.mipmaps.length:1}function U(F){let b=F.target;b.removeEventListener("dispose",U),S(b),b.isVideoTexture&&d.delete(b),b.isHTMLTexture&&m.delete(b)}function p(F){let b=F.target;b.removeEventListener("dispose",p),R(b)}function S(F){let b=i.get(F);if(b.__webglInit===void 0)return;let Y=F.source,te=g.get(Y);if(te){let ae=te[b.__cacheKey];ae.usedTimes--,ae.usedTimes===0&&y(F),Object.keys(te).length===0&&g.delete(Y)}i.remove(F)}function y(F){let b=i.get(F);n.deleteTexture(b.__webglTexture);let Y=F.source,te=g.get(Y);delete te[b.__cacheKey],o.memory.textures--}function R(F){let b=i.get(F);if(F.depthTexture&&(F.depthTexture.dispose(),i.remove(F.depthTexture)),F.isWebGLCubeRenderTarget)for(let te=0;te<6;te++){if(Array.isArray(b.__webglFramebuffer[te]))for(let ae=0;ae<b.__webglFramebuffer[te].length;ae++)n.deleteFramebuffer(b.__webglFramebuffer[te][ae]);else n.deleteFramebuffer(b.__webglFramebuffer[te]);b.__webglDepthbuffer&&n.deleteRenderbuffer(b.__webglDepthbuffer[te])}else{if(Array.isArray(b.__webglFramebuffer))for(let te=0;te<b.__webglFramebuffer.length;te++)n.deleteFramebuffer(b.__webglFramebuffer[te]);else n.deleteFramebuffer(b.__webglFramebuffer);if(b.__webglDepthbuffer&&n.deleteRenderbuffer(b.__webglDepthbuffer),b.__webglMultisampledFramebuffer&&n.deleteFramebuffer(b.__webglMultisampledFramebuffer),b.__webglColorRenderbuffer)for(let te=0;te<b.__webglColorRenderbuffer.length;te++)b.__webglColorRenderbuffer[te]&&n.deleteRenderbuffer(b.__webglColorRenderbuffer[te]);b.__webglDepthRenderbuffer&&n.deleteRenderbuffer(b.__webglDepthRenderbuffer)}let Y=F.textures;for(let te=0,ae=Y.length;te<ae;te++){let ve=i.get(Y[te]);ve.__webglTexture&&(n.deleteTexture(ve.__webglTexture),o.memory.textures--),i.remove(Y[te])}i.remove(F)}let I=0;function k(){I=0}function D(){return I}function N(F){I=F}function H(){let F=I;return F>=r.maxTextures&&$e("WebGLTextures: Trying to use "+(F+1)+" texture units while this GPU supports only "+r.maxTextures),I+=1,F}function $(F){let b=[];return b.push(F.wrapS),b.push(F.wrapT),b.push(F.wrapR||0),b.push(F.magFilter),b.push(F.minFilter),b.push(F.anisotropy),b.push(F.internalFormat),b.push(F.format),b.push(F.type),b.push(F.generateMipmaps),b.push(F.premultiplyAlpha),b.push(F.flipY),b.push(F.unpackAlignment),b.push(F.colorSpace),b.join()}function ee(F,b){let Y=i.get(F);if(F.isVideoTexture&&W(F),F.isRenderTargetTexture===!1&&F.isExternalTexture!==!0&&F.version>0&&Y.__version!==F.version){let te=F.image;if(te===null)$e("WebGLRenderer: Texture marked for update but no image data found.");else if(te.complete===!1)$e("WebGLRenderer: Texture marked for update but image is incomplete");else{be(Y,F,b);return}}else F.isExternalTexture&&(Y.__webglTexture=F.sourceTexture?F.sourceTexture:null);t.bindTexture(n.TEXTURE_2D,Y.__webglTexture,n.TEXTURE0+b)}function J(F,b){let Y=i.get(F);if(F.isRenderTargetTexture===!1&&F.version>0&&Y.__version!==F.version){be(Y,F,b);return}else F.isExternalTexture&&(Y.__webglTexture=F.sourceTexture?F.sourceTexture:null);t.bindTexture(n.TEXTURE_2D_ARRAY,Y.__webglTexture,n.TEXTURE0+b)}function ie(F,b){let Y=i.get(F);if(F.isRenderTargetTexture===!1&&F.version>0&&Y.__version!==F.version){be(Y,F,b);return}t.bindTexture(n.TEXTURE_3D,Y.__webglTexture,n.TEXTURE0+b)}function ce(F,b){let Y=i.get(F);if(F.isCubeDepthTexture!==!0&&F.version>0&&Y.__version!==F.version){Ge(Y,F,b);return}t.bindTexture(n.TEXTURE_CUBE_MAP,Y.__webglTexture,n.TEXTURE0+b)}let we={[dr]:n.REPEAT,[ui]:n.CLAMP_TO_EDGE,[aa]:n.MIRRORED_REPEAT},pe={[Qt]:n.NEAREST,[Qh]:n.NEAREST_MIPMAP_NEAREST,[io]:n.NEAREST_MIPMAP_LINEAR,[xn]:n.LINEAR,[Pa]:n.LINEAR_MIPMAP_NEAREST,[ji]:n.LINEAR_MIPMAP_LINEAR},Pe={[id]:n.NEVER,[ld]:n.ALWAYS,[rd]:n.LESS,[ml]:n.LEQUAL,[sd]:n.EQUAL,[gl]:n.GEQUAL,[od]:n.GREATER,[ad]:n.NOTEQUAL};function Ve(F,b){if(b.type===$n&&e.has("OES_texture_float_linear")===!1&&(b.magFilter===xn||b.magFilter===Pa||b.magFilter===io||b.magFilter===ji||b.minFilter===xn||b.minFilter===Pa||b.minFilter===io||b.minFilter===ji)&&$e("WebGLRenderer: Unable to use linear filtering with floating point textures. OES_texture_float_linear not supported on this device."),n.texParameteri(F,n.TEXTURE_WRAP_S,we[b.wrapS]),n.texParameteri(F,n.TEXTURE_WRAP_T,we[b.wrapT]),(F===n.TEXTURE_3D||F===n.TEXTURE_2D_ARRAY)&&n.texParameteri(F,n.TEXTURE_WRAP_R,we[b.wrapR]),n.texParameteri(F,n.TEXTURE_MAG_FILTER,pe[b.magFilter]),n.texParameteri(F,n.TEXTURE_MIN_FILTER,pe[b.minFilter]),b.compareFunction&&(n.texParameteri(F,n.TEXTURE_COMPARE_MODE,n.COMPARE_REF_TO_TEXTURE),n.texParameteri(F,n.TEXTURE_COMPARE_FUNC,Pe[b.compareFunction])),e.has("EXT_texture_filter_anisotropic")===!0){if(b.magFilter===Qt||b.minFilter!==io&&b.minFilter!==ji||b.type===$n&&e.has("OES_texture_float_linear")===!1)return;if(b.anisotropy>1||i.get(b).__currentAnisotropy){let Y=e.get("EXT_texture_filter_anisotropic");n.texParameterf(F,Y.TEXTURE_MAX_ANISOTROPY_EXT,Math.min(b.anisotropy,r.getMaxAnisotropy())),i.get(b).__currentAnisotropy=b.anisotropy}}}function Oe(F,b){let Y=!1;F.__webglInit===void 0&&(F.__webglInit=!0,b.addEventListener("dispose",U));let te=b.source,ae=g.get(te);ae===void 0&&(ae={},g.set(te,ae));let ve=$(b);if(ve!==F.__cacheKey){ae[ve]===void 0&&(ae[ve]={texture:n.createTexture(),usedTimes:0},o.memory.textures++,Y=!0),ae[ve].usedTimes++;let ye=ae[F.__cacheKey];ye!==void 0&&(ae[F.__cacheKey].usedTimes--,ye.usedTimes===0&&y(b)),F.__cacheKey=ve,F.__webglTexture=ae[ve].texture}return Y}function oe(F,b,Y){return Math.floor(Math.floor(F/Y)/b)}function le(F,b,Y,te){let ve=F.updateRanges;if(ve.length===0)t.texSubImage2D(n.TEXTURE_2D,0,0,0,b.width,b.height,Y,te,b.data);else{ve.sort((z,X)=>z.start-X.start);let ye=0;for(let z=1;z<ve.length;z++){let X=ve[ye],Z=ve[z],re=X.start+X.count,fe=oe(Z.start,b.width,4),me=oe(X.start,b.width,4);Z.start<=re+1&&fe===me&&oe(Z.start+Z.count-1,b.width,4)===fe?X.count=Math.max(X.count,Z.start+Z.count-X.start):(++ye,ve[ye]=Z)}ve.length=ye+1;let C=t.getParameter(n.UNPACK_ROW_LENGTH),L=t.getParameter(n.UNPACK_SKIP_PIXELS),O=t.getParameter(n.UNPACK_SKIP_ROWS);t.pixelStorei(n.UNPACK_ROW_LENGTH,b.width);for(let z=0,X=ve.length;z<X;z++){let Z=ve[z],re=Math.floor(Z.start/4),fe=Math.ceil(Z.count/4),me=re%b.width,B=Math.floor(re/b.width),ue=fe,se=1;t.pixelStorei(n.UNPACK_SKIP_PIXELS,me),t.pixelStorei(n.UNPACK_SKIP_ROWS,B),t.texSubImage2D(n.TEXTURE_2D,0,me,B,ue,se,Y,te,b.data)}F.clearUpdateRanges(),t.pixelStorei(n.UNPACK_ROW_LENGTH,C),t.pixelStorei(n.UNPACK_SKIP_PIXELS,L),t.pixelStorei(n.UNPACK_SKIP_ROWS,O)}}function be(F,b,Y){let te=n.TEXTURE_2D;(b.isDataArrayTexture||b.isCompressedArrayTexture)&&(te=n.TEXTURE_2D_ARRAY),b.isData3DTexture&&(te=n.TEXTURE_3D);let ae=Oe(F,b),ve=b.source;t.bindTexture(te,F.__webglTexture,n.TEXTURE0+Y);let ye=i.get(ve);if(ve.version!==ye.__version||ae===!0){if(t.activeTexture(n.TEXTURE0+Y),(typeof ImageBitmap<"u"&&b.image instanceof ImageBitmap)===!1){let se=ut.getPrimaries(ut.workingColorSpace),ge=b.colorSpace===Ai?null:ut.getPrimaries(b.colorSpace),_e=b.colorSpace===Ai||se===ge?n.NONE:n.BROWSER_DEFAULT_WEBGL;t.pixelStorei(n.UNPACK_FLIP_Y_WEBGL,b.flipY),t.pixelStorei(n.UNPACK_PREMULTIPLY_ALPHA_WEBGL,b.premultiplyAlpha),t.pixelStorei(n.UNPACK_COLORSPACE_CONVERSION_WEBGL,_e)}t.pixelStorei(n.UNPACK_ALIGNMENT,b.unpackAlignment);let L=f(b.image,!1,r.maxTextureSize);L=tn(b,L);let O=s.convert(b.format,b.colorSpace),z=s.convert(b.type),X=w(b.internalFormat,O,z,b.normalized,b.colorSpace,b.isVideoTexture);Ve(te,b);let Z,re=b.mipmaps,fe=b.isVideoTexture!==!0,me=ye.__version===void 0||ae===!0,B=ve.dataReady,ue=E(b,L);if(b.isDepthTexture)X=T(b.format===Qi,b.type),me&&(fe?t.texStorage2D(n.TEXTURE_2D,1,X,L.width,L.height):t.texImage2D(n.TEXTURE_2D,0,X,L.width,L.height,0,O,z,null));else if(b.isDataTexture)if(re.length>0){fe&&me&&t.texStorage2D(n.TEXTURE_2D,ue,X,re[0].width,re[0].height);for(let se=0,ge=re.length;se<ge;se++)Z=re[se],fe?B&&t.texSubImage2D(n.TEXTURE_2D,se,0,0,Z.width,Z.height,O,z,Z.data):t.texImage2D(n.TEXTURE_2D,se,X,Z.width,Z.height,0,O,z,Z.data);b.generateMipmaps=!1}else fe?(me&&t.texStorage2D(n.TEXTURE_2D,ue,X,L.width,L.height),B&&le(b,L,O,z)):t.texImage2D(n.TEXTURE_2D,0,X,L.width,L.height,0,O,z,L.data);else if(b.isCompressedTexture)if(b.isCompressedArrayTexture){fe&&me&&t.texStorage3D(n.TEXTURE_2D_ARRAY,ue,X,re[0].width,re[0].height,L.depth);for(let se=0,ge=re.length;se<ge;se++)if(Z=re[se],b.format!==Fn)if(O!==null)if(fe){if(B)if(b.layerUpdates.size>0){let _e=eu(Z.width,Z.height,b.format,b.type);for(let he of b.layerUpdates){let Ce=Z.data.subarray(he*_e/Z.data.BYTES_PER_ELEMENT,(he+1)*_e/Z.data.BYTES_PER_ELEMENT);t.compressedTexSubImage3D(n.TEXTURE_2D_ARRAY,se,0,0,he,Z.width,Z.height,1,O,Ce)}}else t.compressedTexSubImage3D(n.TEXTURE_2D_ARRAY,se,0,0,0,Z.width,Z.height,L.depth,O,Z.data)}else t.compressedTexImage3D(n.TEXTURE_2D_ARRAY,se,X,Z.width,Z.height,L.depth,0,Z.data,0,0);else $e("WebGLRenderer: Attempt to load unsupported compressed texture format in .uploadTexture()");else fe?B&&t.texSubImage3D(n.TEXTURE_2D_ARRAY,se,0,0,0,Z.width,Z.height,L.depth,O,z,Z.data):t.texImage3D(n.TEXTURE_2D_ARRAY,se,X,Z.width,Z.height,L.depth,0,O,z,Z.data);b.layerUpdates.size>0&&b.clearLayerUpdates()}else{fe&&me&&t.texStorage2D(n.TEXTURE_2D,ue,X,re[0].width,re[0].height);for(let se=0,ge=re.length;se<ge;se++)Z=re[se],b.format!==Fn?O!==null?fe?B&&t.compressedTexSubImage2D(n.TEXTURE_2D,se,0,0,Z.width,Z.height,O,Z.data):t.compressedTexImage2D(n.TEXTURE_2D,se,X,Z.width,Z.height,0,Z.data):$e("WebGLRenderer: Attempt to load unsupported compressed texture format in .uploadTexture()"):fe?B&&t.texSubImage2D(n.TEXTURE_2D,se,0,0,Z.width,Z.height,O,z,Z.data):t.texImage2D(n.TEXTURE_2D,se,X,Z.width,Z.height,0,O,z,Z.data)}else if(b.isDataArrayTexture)if(fe){if(me&&t.texStorage3D(n.TEXTURE_2D_ARRAY,ue,X,L.width,L.height,L.depth),B)if(b.layerUpdates.size>0){let se=eu(L.width,L.height,b.format,b.type);for(let ge of b.layerUpdates){let _e=L.data.subarray(ge*se/L.data.BYTES_PER_ELEMENT,(ge+1)*se/L.data.BYTES_PER_ELEMENT);t.texSubImage3D(n.TEXTURE_2D_ARRAY,0,0,0,ge,L.width,L.height,1,O,z,_e)}b.clearLayerUpdates()}else t.texSubImage3D(n.TEXTURE_2D_ARRAY,0,0,0,0,L.width,L.height,L.depth,O,z,L.data)}else t.texImage3D(n.TEXTURE_2D_ARRAY,0,X,L.width,L.height,L.depth,0,O,z,L.data);else if(b.isData3DTexture)fe?(me&&t.texStorage3D(n.TEXTURE_3D,ue,X,L.width,L.height,L.depth),B&&t.texSubImage3D(n.TEXTURE_3D,0,0,0,0,L.width,L.height,L.depth,O,z,L.data)):t.texImage3D(n.TEXTURE_3D,0,X,L.width,L.height,L.depth,0,O,z,L.data);else if(b.isFramebufferTexture){if(me)if(fe)t.texStorage2D(n.TEXTURE_2D,ue,X,L.width,L.height);else{let se=L.width,ge=L.height;for(let _e=0;_e<ue;_e++)t.texImage2D(n.TEXTURE_2D,_e,X,se,ge,0,O,z,null),se>>=1,ge>>=1}}else if(b.isHTMLTexture){if("texElementImage2D"in n){let se=n.canvas;if(se.hasAttribute("layoutsubtree")||se.setAttribute("layoutsubtree","true"),L.parentNode!==se){se.appendChild(L),m.add(b),se.onpaint=ge=>{let _e=ge.changedElements;for(let he of m)_e.includes(he.image)&&(he.needsUpdate=!0)},se.requestPaint();return}if(n.texElementImage2D.length===3)n.texElementImage2D(n.TEXTURE_2D,n.RGBA8,L);else{let _e=n.RGBA,he=n.RGBA,Ce=n.UNSIGNED_BYTE;n.texElementImage2D(n.TEXTURE_2D,0,_e,he,Ce,L)}n.texParameteri(n.TEXTURE_2D,n.TEXTURE_MIN_FILTER,n.LINEAR),n.texParameteri(n.TEXTURE_2D,n.TEXTURE_WRAP_S,n.CLAMP_TO_EDGE),n.texParameteri(n.TEXTURE_2D,n.TEXTURE_WRAP_T,n.CLAMP_TO_EDGE)}}else if(re.length>0){if(fe&&me){let se=ct(re[0]);t.texStorage2D(n.TEXTURE_2D,ue,X,se.width,se.height)}for(let se=0,ge=re.length;se<ge;se++)Z=re[se],fe?B&&t.texSubImage2D(n.TEXTURE_2D,se,0,0,O,z,Z):t.texImage2D(n.TEXTURE_2D,se,X,O,z,Z);b.generateMipmaps=!1}else if(fe){if(me){let se=ct(L);t.texStorage2D(n.TEXTURE_2D,ue,X,se.width,se.height)}B&&t.texSubImage2D(n.TEXTURE_2D,0,0,0,O,z,L)}else t.texImage2D(n.TEXTURE_2D,0,X,O,z,L);h(b)&&M(te),ye.__version=ve.version,b.onUpdate&&b.onUpdate(b)}F.__version=b.version}function Ge(F,b,Y){if(b.image.length!==6)return;let te=Oe(F,b),ae=b.source;t.bindTexture(n.TEXTURE_CUBE_MAP,F.__webglTexture,n.TEXTURE0+Y);let ve=i.get(ae);if(ae.version!==ve.__version||te===!0){t.activeTexture(n.TEXTURE0+Y);let ye=ut.getPrimaries(ut.workingColorSpace),C=b.colorSpace===Ai?null:ut.getPrimaries(b.colorSpace),L=b.colorSpace===Ai||ye===C?n.NONE:n.BROWSER_DEFAULT_WEBGL;t.pixelStorei(n.UNPACK_FLIP_Y_WEBGL,b.flipY),t.pixelStorei(n.UNPACK_PREMULTIPLY_ALPHA_WEBGL,b.premultiplyAlpha),t.pixelStorei(n.UNPACK_ALIGNMENT,b.unpackAlignment),t.pixelStorei(n.UNPACK_COLORSPACE_CONVERSION_WEBGL,L);let O=b.isCompressedTexture||b.image[0].isCompressedTexture,z=b.image[0]&&b.image[0].isDataTexture,X=[];for(let he=0;he<6;he++)!O&&!z?X[he]=f(b.image[he],!0,r.maxCubemapSize):X[he]=z?b.image[he].image:b.image[he],X[he]=tn(b,X[he]);let Z=X[0],re=s.convert(b.format,b.colorSpace),fe=s.convert(b.type),me=w(b.internalFormat,re,fe,b.normalized,b.colorSpace),B=b.isVideoTexture!==!0,ue=ve.__version===void 0||te===!0,se=ae.dataReady,ge=E(b,Z);Ve(n.TEXTURE_CUBE_MAP,b);let _e;if(O){B&&ue&&t.texStorage2D(n.TEXTURE_CUBE_MAP,ge,me,Z.width,Z.height);for(let he=0;he<6;he++){_e=X[he].mipmaps;for(let Ce=0;Ce<_e.length;Ce++){let Le=_e[Ce];b.format!==Fn?re!==null?B?se&&t.compressedTexSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,Ce,0,0,Le.width,Le.height,re,Le.data):t.compressedTexImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,Ce,me,Le.width,Le.height,0,Le.data):$e("WebGLRenderer: Attempt to load unsupported compressed texture format in .setTextureCube()"):B?se&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,Ce,0,0,Le.width,Le.height,re,fe,Le.data):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,Ce,me,Le.width,Le.height,0,re,fe,Le.data)}}}else{if(_e=b.mipmaps,B&&ue){_e.length>0&&ge++;let he=ct(X[0]);t.texStorage2D(n.TEXTURE_CUBE_MAP,ge,me,he.width,he.height)}for(let he=0;he<6;he++)if(z){B?se&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,0,0,0,X[he].width,X[he].height,re,fe,X[he].data):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,0,me,X[he].width,X[he].height,0,re,fe,X[he].data);for(let Ce=0;Ce<_e.length;Ce++){let _t=_e[Ce].image[he].image;B?se&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,Ce+1,0,0,_t.width,_t.height,re,fe,_t.data):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,Ce+1,me,_t.width,_t.height,0,re,fe,_t.data)}}else{B?se&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,0,0,0,re,fe,X[he]):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,0,me,re,fe,X[he]);for(let Ce=0;Ce<_e.length;Ce++){let Le=_e[Ce];B?se&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,Ce+1,0,0,re,fe,Le.image[he]):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+he,Ce+1,me,re,fe,Le.image[he])}}}h(b)&&M(n.TEXTURE_CUBE_MAP),ve.__version=ae.version,b.onUpdate&&b.onUpdate(b)}F.__version=b.version}function Me(F,b,Y,te,ae,ve){let ye=s.convert(Y.format,Y.colorSpace),C=s.convert(Y.type),L=w(Y.internalFormat,ye,C,Y.normalized,Y.colorSpace),O=i.get(b),z=i.get(Y);if(z.__renderTarget=b,!O.__hasExternalTextures){let X=Math.max(1,b.width>>ve),Z=Math.max(1,b.height>>ve);ae===n.TEXTURE_3D||ae===n.TEXTURE_2D_ARRAY?t.texImage3D(ae,ve,L,X,Z,b.depth,0,ye,C,null):t.texImage2D(ae,ve,L,X,Z,0,ye,C,null)}t.bindFramebuffer(n.FRAMEBUFFER,F),Ot(b)?a.framebufferTexture2DMultisampleEXT(n.FRAMEBUFFER,te,ae,z.__webglTexture,0,Pt(b)):(ae===n.TEXTURE_2D||ae>=n.TEXTURE_CUBE_MAP_POSITIVE_X&&ae<=n.TEXTURE_CUBE_MAP_NEGATIVE_Z)&&n.framebufferTexture2D(n.FRAMEBUFFER,te,ae,z.__webglTexture,ve),t.bindFramebuffer(n.FRAMEBUFFER,null)}function ze(F,b,Y){if(n.bindRenderbuffer(n.RENDERBUFFER,F),b.depthBuffer){let te=b.depthTexture,ae=te&&te.isDepthTexture?te.type:null,ve=T(b.stencilBuffer,ae),ye=b.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT;Ot(b)?a.renderbufferStorageMultisampleEXT(n.RENDERBUFFER,Pt(b),ve,b.width,b.height):Y?n.renderbufferStorageMultisample(n.RENDERBUFFER,Pt(b),ve,b.width,b.height):n.renderbufferStorage(n.RENDERBUFFER,ve,b.width,b.height),n.framebufferRenderbuffer(n.FRAMEBUFFER,ye,n.RENDERBUFFER,F)}else{let te=b.textures;for(let ae=0;ae<te.length;ae++){let ve=te[ae],ye=s.convert(ve.format,ve.colorSpace),C=s.convert(ve.type),L=w(ve.internalFormat,ye,C,ve.normalized,ve.colorSpace);Ot(b)?a.renderbufferStorageMultisampleEXT(n.RENDERBUFFER,Pt(b),L,b.width,b.height):Y?n.renderbufferStorageMultisample(n.RENDERBUFFER,Pt(b),L,b.width,b.height):n.renderbufferStorage(n.RENDERBUFFER,L,b.width,b.height)}}n.bindRenderbuffer(n.RENDERBUFFER,null)}function Ut(F,b,Y){let te=b.isWebGLCubeRenderTarget===!0;if(t.bindFramebuffer(n.FRAMEBUFFER,F),!(b.depthTexture&&b.depthTexture.isDepthTexture))throw new Error("THREE.WebGLTextures: renderTarget.depthTexture must be an instance of THREE.DepthTexture.");let ae=i.get(b.depthTexture);if(ae.__renderTarget=b,(!ae.__webglTexture||b.depthTexture.image.width!==b.width||b.depthTexture.image.height!==b.height)&&(b.depthTexture.image.width=b.width,b.depthTexture.image.height=b.height,b.depthTexture.needsUpdate=!0),te){if(ae.__webglInit===void 0&&(ae.__webglInit=!0,b.depthTexture.addEventListener("dispose",U)),ae.__webglTexture===void 0){ae.__webglTexture=n.createTexture(),t.bindTexture(n.TEXTURE_CUBE_MAP,ae.__webglTexture),Ve(n.TEXTURE_CUBE_MAP,b.depthTexture);let O=s.convert(b.depthTexture.format),z=s.convert(b.depthTexture.type),X;b.depthTexture.format===hi?X=n.DEPTH_COMPONENT24:b.depthTexture.format===Qi&&(X=n.DEPTH24_STENCIL8);for(let Z=0;Z<6;Z++)n.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+Z,0,X,b.width,b.height,0,O,z,null)}}else ee(b.depthTexture,0);let ve=ae.__webglTexture,ye=Pt(b),C=te?n.TEXTURE_CUBE_MAP_POSITIVE_X+Y:n.TEXTURE_2D,L=b.depthTexture.format===Qi?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT;if(b.depthTexture.format===hi)Ot(b)?a.framebufferTexture2DMultisampleEXT(n.FRAMEBUFFER,L,C,ve,0,ye):n.framebufferTexture2D(n.FRAMEBUFFER,L,C,ve,0);else if(b.depthTexture.format===Qi)Ot(b)?a.framebufferTexture2DMultisampleEXT(n.FRAMEBUFFER,L,C,ve,0,ye):n.framebufferTexture2D(n.FRAMEBUFFER,L,C,ve,0);else throw new Error("THREE.WebGLTextures: Unknown depthTexture format.")}function Je(F){let b=i.get(F),Y=F.isWebGLCubeRenderTarget===!0;if(b.__boundDepthTexture!==F.depthTexture){let te=F.depthTexture;if(b.__depthDisposeCallback&&b.__depthDisposeCallback(),te){let ae=()=>{delete b.__boundDepthTexture,delete b.__depthDisposeCallback,te.removeEventListener("dispose",ae)};te.addEventListener("dispose",ae),b.__depthDisposeCallback=ae}b.__boundDepthTexture=te}if(F.depthTexture&&!b.__autoAllocateDepthBuffer)if(Y)for(let te=0;te<6;te++)Ut(b.__webglFramebuffer[te],F,te);else{let te=F.texture.mipmaps;te&&te.length>0?Ut(b.__webglFramebuffer[0],F,0):Ut(b.__webglFramebuffer,F,0)}else if(Y){b.__webglDepthbuffer=[];for(let te=0;te<6;te++)if(t.bindFramebuffer(n.FRAMEBUFFER,b.__webglFramebuffer[te]),b.__webglDepthbuffer[te]===void 0)b.__webglDepthbuffer[te]=n.createRenderbuffer(),ze(b.__webglDepthbuffer[te],F,!1);else{let ae=F.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT,ve=b.__webglDepthbuffer[te];n.bindRenderbuffer(n.RENDERBUFFER,ve),n.framebufferRenderbuffer(n.FRAMEBUFFER,ae,n.RENDERBUFFER,ve)}}else{let te=F.texture.mipmaps;if(te&&te.length>0?t.bindFramebuffer(n.FRAMEBUFFER,b.__webglFramebuffer[0]):t.bindFramebuffer(n.FRAMEBUFFER,b.__webglFramebuffer),b.__webglDepthbuffer===void 0)b.__webglDepthbuffer=n.createRenderbuffer(),ze(b.__webglDepthbuffer,F,!1);else{let ae=F.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT,ve=b.__webglDepthbuffer;n.bindRenderbuffer(n.RENDERBUFFER,ve),n.framebufferRenderbuffer(n.FRAMEBUFFER,ae,n.RENDERBUFFER,ve)}}t.bindFramebuffer(n.FRAMEBUFFER,null)}function st(F,b,Y){let te=i.get(F);b!==void 0&&Me(te.__webglFramebuffer,F,F.texture,n.COLOR_ATTACHMENT0,n.TEXTURE_2D,0),Y!==void 0&&Je(F)}function gt(F){let b=F.texture,Y=i.get(F),te=i.get(b);F.addEventListener("dispose",p);let ae=F.textures,ve=F.isWebGLCubeRenderTarget===!0,ye=ae.length>1;if(ye||(te.__webglTexture===void 0&&(te.__webglTexture=n.createTexture()),te.__version=b.version,o.memory.textures++),ve){Y.__webglFramebuffer=[];for(let C=0;C<6;C++)if(b.mipmaps&&b.mipmaps.length>0){Y.__webglFramebuffer[C]=[];for(let L=0;L<b.mipmaps.length;L++)Y.__webglFramebuffer[C][L]=n.createFramebuffer()}else Y.__webglFramebuffer[C]=n.createFramebuffer()}else{if(b.mipmaps&&b.mipmaps.length>0){Y.__webglFramebuffer=[];for(let C=0;C<b.mipmaps.length;C++)Y.__webglFramebuffer[C]=n.createFramebuffer()}else Y.__webglFramebuffer=n.createFramebuffer();if(ye)for(let C=0,L=ae.length;C<L;C++){let O=i.get(ae[C]);O.__webglTexture===void 0&&(O.__webglTexture=n.createTexture(),o.memory.textures++)}if(F.samples>0&&Ot(F)===!1){Y.__webglMultisampledFramebuffer=n.createFramebuffer(),Y.__webglColorRenderbuffer=[],t.bindFramebuffer(n.FRAMEBUFFER,Y.__webglMultisampledFramebuffer);for(let C=0;C<ae.length;C++){let L=ae[C];Y.__webglColorRenderbuffer[C]=n.createRenderbuffer(),n.bindRenderbuffer(n.RENDERBUFFER,Y.__webglColorRenderbuffer[C]);let O=s.convert(L.format,L.colorSpace),z=s.convert(L.type),X=w(L.internalFormat,O,z,L.normalized,L.colorSpace,F.isXRRenderTarget===!0),Z=Pt(F);n.renderbufferStorageMultisample(n.RENDERBUFFER,Z,X,F.width,F.height),n.framebufferRenderbuffer(n.FRAMEBUFFER,n.COLOR_ATTACHMENT0+C,n.RENDERBUFFER,Y.__webglColorRenderbuffer[C])}n.bindRenderbuffer(n.RENDERBUFFER,null),F.depthBuffer&&(Y.__webglDepthRenderbuffer=n.createRenderbuffer(),ze(Y.__webglDepthRenderbuffer,F,!0)),t.bindFramebuffer(n.FRAMEBUFFER,null)}}if(ve){t.bindTexture(n.TEXTURE_CUBE_MAP,te.__webglTexture),Ve(n.TEXTURE_CUBE_MAP,b);for(let C=0;C<6;C++)if(b.mipmaps&&b.mipmaps.length>0)for(let L=0;L<b.mipmaps.length;L++)Me(Y.__webglFramebuffer[C][L],F,b,n.COLOR_ATTACHMENT0,n.TEXTURE_CUBE_MAP_POSITIVE_X+C,L);else Me(Y.__webglFramebuffer[C],F,b,n.COLOR_ATTACHMENT0,n.TEXTURE_CUBE_MAP_POSITIVE_X+C,0);h(b)&&M(n.TEXTURE_CUBE_MAP),t.unbindTexture()}else if(ye){for(let C=0,L=ae.length;C<L;C++){let O=ae[C],z=i.get(O),X=n.TEXTURE_2D;(F.isWebGL3DRenderTarget||F.isWebGLArrayRenderTarget)&&(X=F.isWebGL3DRenderTarget?n.TEXTURE_3D:n.TEXTURE_2D_ARRAY),t.bindTexture(X,z.__webglTexture),Ve(X,O),Me(Y.__webglFramebuffer,F,O,n.COLOR_ATTACHMENT0+C,X,0),h(O)&&M(X)}t.unbindTexture()}else{let C=n.TEXTURE_2D;if((F.isWebGL3DRenderTarget||F.isWebGLArrayRenderTarget)&&(C=F.isWebGL3DRenderTarget?n.TEXTURE_3D:n.TEXTURE_2D_ARRAY),t.bindTexture(C,te.__webglTexture),Ve(C,b),b.mipmaps&&b.mipmaps.length>0)for(let L=0;L<b.mipmaps.length;L++)Me(Y.__webglFramebuffer[L],F,b,n.COLOR_ATTACHMENT0,C,L);else Me(Y.__webglFramebuffer,F,b,n.COLOR_ATTACHMENT0,C,0);h(b)&&M(C),t.unbindTexture()}F.depthBuffer&&Je(F)}function tt(F){let b=F.textures;for(let Y=0,te=b.length;Y<te;Y++){let ae=b[Y];if(h(ae)){let ve=P(F),ye=i.get(ae).__webglTexture;t.bindTexture(ve,ye),M(ve),t.unbindTexture()}}}let Tt=[],Ft=[];function Wt(F){if(F.samples>0){if(Ot(F)===!1){let b=F.textures,Y=F.width,te=F.height,ae=n.COLOR_BUFFER_BIT,ve=F.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT,ye=i.get(F),C=b.length>1;if(C)for(let O=0;O<b.length;O++)t.bindFramebuffer(n.FRAMEBUFFER,ye.__webglMultisampledFramebuffer),n.framebufferRenderbuffer(n.FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.RENDERBUFFER,null),t.bindFramebuffer(n.FRAMEBUFFER,ye.__webglFramebuffer),n.framebufferTexture2D(n.DRAW_FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.TEXTURE_2D,null,0);t.bindFramebuffer(n.READ_FRAMEBUFFER,ye.__webglMultisampledFramebuffer);let L=F.texture.mipmaps;L&&L.length>0?t.bindFramebuffer(n.DRAW_FRAMEBUFFER,ye.__webglFramebuffer[0]):t.bindFramebuffer(n.DRAW_FRAMEBUFFER,ye.__webglFramebuffer);for(let O=0;O<b.length;O++){if(F.resolveDepthBuffer&&(F.depthBuffer&&(ae|=n.DEPTH_BUFFER_BIT),F.stencilBuffer&&F.resolveStencilBuffer&&(ae|=n.STENCIL_BUFFER_BIT)),C){n.framebufferRenderbuffer(n.READ_FRAMEBUFFER,n.COLOR_ATTACHMENT0,n.RENDERBUFFER,ye.__webglColorRenderbuffer[O]);let z=i.get(b[O]).__webglTexture;n.framebufferTexture2D(n.DRAW_FRAMEBUFFER,n.COLOR_ATTACHMENT0,n.TEXTURE_2D,z,0)}n.blitFramebuffer(0,0,Y,te,0,0,Y,te,ae,n.NEAREST),l===!0&&(Tt.length=0,Ft.length=0,Tt.push(n.COLOR_ATTACHMENT0+O),F.depthBuffer&&F.storeMultisampledDepthBuffer===!1&&(Tt.push(ve),Ft.push(ve),n.invalidateFramebuffer(n.DRAW_FRAMEBUFFER,Ft)),n.invalidateFramebuffer(n.READ_FRAMEBUFFER,Tt))}if(t.bindFramebuffer(n.READ_FRAMEBUFFER,null),t.bindFramebuffer(n.DRAW_FRAMEBUFFER,null),C)for(let O=0;O<b.length;O++){t.bindFramebuffer(n.FRAMEBUFFER,ye.__webglMultisampledFramebuffer),n.framebufferRenderbuffer(n.FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.RENDERBUFFER,ye.__webglColorRenderbuffer[O]);let z=i.get(b[O]).__webglTexture;t.bindFramebuffer(n.FRAMEBUFFER,ye.__webglFramebuffer),n.framebufferTexture2D(n.DRAW_FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.TEXTURE_2D,z,0)}t.bindFramebuffer(n.DRAW_FRAMEBUFFER,ye.__webglMultisampledFramebuffer)}else if(F.depthBuffer&&F.storeMultisampledDepthBuffer===!1&&l){let b=F.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT;n.invalidateFramebuffer(n.DRAW_FRAMEBUFFER,[b])}}}function Pt(F){return Math.min(r.maxSamples,F.samples)}function Ot(F){let b=i.get(F);return F.samples>0&&e.has("WEBGL_multisampled_render_to_texture")===!0&&b.__useRenderToTexture!==!1}function W(F){let b=o.render.frame;d.get(F)!==b&&(d.set(F,b),F.update())}function tn(F,b){let Y=F.colorSpace,te=F.format,ae=F.type;return F.isCompressedTexture===!0||F.isVideoTexture===!0||Y!==Ps&&Y!==Ai&&(ut.getTransfer(Y)===St?(te!==Fn||ae!==Un)&&$e("WebGLTextures: sRGB encoded textures have to use RGBAFormat and UnsignedByteType."):Ze("WebGLTextures: Unsupported texture color space:",Y)),b}function ct(F){return typeof HTMLImageElement<"u"&&F instanceof HTMLImageElement?(c.width=F.naturalWidth||F.width,c.height=F.naturalHeight||F.height):typeof VideoFrame<"u"&&F instanceof VideoFrame?(c.width=F.displayWidth,c.height=F.displayHeight):(c.width=F.width,c.height=F.height),c}this.allocateTextureUnit=H,this.resetTextureUnits=k,this.getTextureUnits=D,this.setTextureUnits=N,this.setTexture2D=ee,this.setTexture2DArray=J,this.setTexture3D=ie,this.setTextureCube=ce,this.rebindTextures=st,this.setupRenderTarget=gt,this.updateRenderTargetMipmap=tt,this.updateMultisampleRenderTarget=Wt,this.setupDepthRenderbuffer=Je,this.setupFrameBufferTexture=Me,this.useMultisampledRTT=Ot,this.isReversedDepthBuffer=function(){return t.buffers.depth.getReversed()}}function Zv(n,e){function t(i,r=Ai){let s,o=ut.getTransfer(r);if(i===Un)return n.UNSIGNED_BYTE;if(i===La)return n.UNSIGNED_SHORT_4_4_4_4;if(i===ka)return n.UNSIGNED_SHORT_5_5_5_1;if(i===Vc)return n.UNSIGNED_INT_5_9_9_9_REV;if(i===Wc)return n.UNSIGNED_INT_10F_11F_11F_REV;if(i===Hc)return n.BYTE;if(i===Gc)return n.SHORT;if(i===as)return n.UNSIGNED_SHORT;if(i===Ia)return n.INT;if(i===ii)return n.UNSIGNED_INT;if(i===$n)return n.FLOAT;if(i===ri)return n.HALF_FLOAT;if(i===Xc)return n.ALPHA;if(i===qc)return n.RGB;if(i===Fn)return n.RGBA;if(i===hi)return n.DEPTH_COMPONENT;if(i===Qi)return n.DEPTH_STENCIL;if(i===Da)return n.RED;if(i===Na)return n.RED_INTEGER;if(i===er)return n.RG;if(i===Ua)return n.RG_INTEGER;if(i===Fa)return n.RGBA_INTEGER;if(i===ro||i===so||i===oo||i===ao)if(o===St)if(s=e.get("WEBGL_compressed_texture_s3tc_srgb"),s!==null){if(i===ro)return s.COMPRESSED_SRGB_S3TC_DXT1_EXT;if(i===so)return s.COMPRESSED_SRGB_ALPHA_S3TC_DXT1_EXT;if(i===oo)return s.COMPRESSED_SRGB_ALPHA_S3TC_DXT3_EXT;if(i===ao)return s.COMPRESSED_SRGB_ALPHA_S3TC_DXT5_EXT}else return null;else if(s=e.get("WEBGL_compressed_texture_s3tc"),s!==null){if(i===ro)return s.COMPRESSED_RGB_S3TC_DXT1_EXT;if(i===so)return s.COMPRESSED_RGBA_S3TC_DXT1_EXT;if(i===oo)return s.COMPRESSED_RGBA_S3TC_DXT3_EXT;if(i===ao)return s.COMPRESSED_RGBA_S3TC_DXT5_EXT}else return null;if(i===Oa||i===Ba||i===za||i===Ha)if(s=e.get("WEBGL_compressed_texture_pvrtc"),s!==null){if(i===Oa)return s.COMPRESSED_RGB_PVRTC_4BPPV1_IMG;if(i===Ba)return s.COMPRESSED_RGB_PVRTC_2BPPV1_IMG;if(i===za)return s.COMPRESSED_RGBA_PVRTC_4BPPV1_IMG;if(i===Ha)return s.COMPRESSED_RGBA_PVRTC_2BPPV1_IMG}else return null;if(i===Ga||i===Va||i===Wa||i===Xa||i===qa||i===lo||i===$a)if(s=e.get("WEBGL_compressed_texture_etc"),s!==null){if(i===Ga||i===Va)return o===St?s.COMPRESSED_SRGB8_ETC2:s.COMPRESSED_RGB8_ETC2;if(i===Wa)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ETC2_EAC:s.COMPRESSED_RGBA8_ETC2_EAC;if(i===Xa)return s.COMPRESSED_R11_EAC;if(i===qa)return s.COMPRESSED_SIGNED_R11_EAC;if(i===lo)return s.COMPRESSED_RG11_EAC;if(i===$a)return s.COMPRESSED_SIGNED_RG11_EAC}else return null;if(i===Ya||i===Za||i===Ka||i===Ja||i===ja||i===Qa||i===el||i===tl||i===nl||i===il||i===rl||i===sl||i===ol||i===al)if(s=e.get("WEBGL_compressed_texture_astc"),s!==null){if(i===Ya)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_4x4_KHR:s.COMPRESSED_RGBA_ASTC_4x4_KHR;if(i===Za)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_5x4_KHR:s.COMPRESSED_RGBA_ASTC_5x4_KHR;if(i===Ka)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_5x5_KHR:s.COMPRESSED_RGBA_ASTC_5x5_KHR;if(i===Ja)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_6x5_KHR:s.COMPRESSED_RGBA_ASTC_6x5_KHR;if(i===ja)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_6x6_KHR:s.COMPRESSED_RGBA_ASTC_6x6_KHR;if(i===Qa)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_8x5_KHR:s.COMPRESSED_RGBA_ASTC_8x5_KHR;if(i===el)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_8x6_KHR:s.COMPRESSED_RGBA_ASTC_8x6_KHR;if(i===tl)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_8x8_KHR:s.COMPRESSED_RGBA_ASTC_8x8_KHR;if(i===nl)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x5_KHR:s.COMPRESSED_RGBA_ASTC_10x5_KHR;if(i===il)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x6_KHR:s.COMPRESSED_RGBA_ASTC_10x6_KHR;if(i===rl)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x8_KHR:s.COMPRESSED_RGBA_ASTC_10x8_KHR;if(i===sl)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x10_KHR:s.COMPRESSED_RGBA_ASTC_10x10_KHR;if(i===ol)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_12x10_KHR:s.COMPRESSED_RGBA_ASTC_12x10_KHR;if(i===al)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_12x12_KHR:s.COMPRESSED_RGBA_ASTC_12x12_KHR}else return null;if(i===ll||i===cl||i===ul)if(s=e.get("EXT_texture_compression_bptc"),s!==null){if(i===ll)return o===St?s.COMPRESSED_SRGB_ALPHA_BPTC_UNORM_EXT:s.COMPRESSED_RGBA_BPTC_UNORM_EXT;if(i===cl)return s.COMPRESSED_RGB_BPTC_SIGNED_FLOAT_EXT;if(i===ul)return s.COMPRESSED_RGB_BPTC_UNSIGNED_FLOAT_EXT}else return null;if(i===hl||i===dl||i===co||i===fl)if(s=e.get("EXT_texture_compression_rgtc"),s!==null){if(i===hl)return s.COMPRESSED_RED_RGTC1_EXT;if(i===dl)return s.COMPRESSED_SIGNED_RED_RGTC1_EXT;if(i===co)return s.COMPRESSED_RED_GREEN_RGTC2_EXT;if(i===fl)return s.COMPRESSED_SIGNED_RED_GREEN_RGTC2_EXT}else return null;return i===ls?n.UNSIGNED_INT_24_8:n[i]!==void 0?n[i]:null}return{convert:t}}var Kv=`
void main() {

	gl_Position = vec4( position, 1.0 );

}`,Jv=`
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

}`,pu=class{constructor(){this.texture=null,this.mesh=null,this.depthNear=0,this.depthFar=0}init(e,t){if(this.texture===null){let i=new Gs(e.texture);(e.depthNear!==t.depthNear||e.depthFar!==t.depthFar)&&(this.depthNear=e.depthNear,this.depthFar=e.depthFar),this.texture=i}}getMesh(e){if(this.texture!==null&&this.mesh===null){let t=e.cameras[0].viewport,i=new Tn({vertexShader:Kv,fragmentShader:Jv,uniforms:{depthColor:{value:this.texture},depthWidth:{value:t.z},depthHeight:{value:t.w}}});this.mesh=new rt(new Vi(20,20),i)}return this.mesh}reset(){this.texture=null,this.mesh=null}getDepthTexture(){return this.texture}},mu=class extends ti{constructor(e,t){super();let i=this,r=null,s=1,o=null,a="local-floor",l=1,c=null,d=null,m=null,u=null,g=null,_=null,v=typeof XRWebGLBinding<"u",f=new pu,h={},M=t.getContextAttributes(),P=null,w=null,T=[],E=[],U=new We,p=null,S=null,y=new gn;y.viewport=new Bt;let R=new gn;R.viewport=new Bt;let I=[y,R],k=new Aa,D=null,N=null;this.cameraAutoUpdate=!0,this.enabled=!1,this.isPresenting=!1,this.getController=function(oe){let le=T[oe];return le===void 0&&(le=new Jr,T[oe]=le),le.getTargetRaySpace()},this.getControllerGrip=function(oe){let le=T[oe];return le===void 0&&(le=new Jr,T[oe]=le),le.getGripSpace()},this.getHand=function(oe){let le=T[oe];return le===void 0&&(le=new Jr,T[oe]=le),le.getHandSpace()};function H(oe){let le=E.indexOf(oe.inputSource);if(le===-1)return;let be=T[le];be!==void 0&&(be.update(oe.inputSource,oe.frame,c||o),be.dispatchEvent({type:oe.type,data:oe.inputSource}))}function $(){r.removeEventListener("select",H),r.removeEventListener("selectstart",H),r.removeEventListener("selectend",H),r.removeEventListener("squeeze",H),r.removeEventListener("squeezestart",H),r.removeEventListener("squeezeend",H),r.removeEventListener("end",$),r.removeEventListener("inputsourceschange",ee);for(let oe=0;oe<T.length;oe++){let le=E[oe];le!==null&&(E[oe]=null,T[oe].disconnect(le))}D=null,N=null,f.reset();for(let oe in h)delete h[oe];if(e.setRenderTarget(P),g=null,u=null,m=null,r=null,w=null,Oe.stop(),i.isPresenting=!1,e.setPixelRatio(p),e.setSize(U.width,U.height,!1),S!==null){let oe=S.camera;oe.fov=S.fov,oe.zoom=S.zoom,oe.updateProjectionMatrix(),S=null}i.dispatchEvent({type:"sessionend"})}this.setFramebufferScaleFactor=function(oe){s=oe,i.isPresenting===!0&&$e("WebXRManager: Cannot change framebuffer scale while presenting.")},this.setReferenceSpaceType=function(oe){a=oe,i.isPresenting===!0&&$e("WebXRManager: Cannot change reference space type while presenting.")},this.getReferenceSpace=function(){return c||o},this.setReferenceSpace=function(oe){c=oe},this.getBaseLayer=function(){return u!==null?u:g},this.getBinding=function(){return m===null&&v&&(m=new XRWebGLBinding(r,t)),m},this.getFrame=function(){return _},this.getSession=function(){return r},this.setSession=async function(oe){if(r=oe,r!==null){if(P=e.getRenderTarget(),r.addEventListener("select",H),r.addEventListener("selectstart",H),r.addEventListener("selectend",H),r.addEventListener("squeeze",H),r.addEventListener("squeezestart",H),r.addEventListener("squeezeend",H),r.addEventListener("end",$),r.addEventListener("inputsourceschange",ee),M.xrCompatible!==!0&&await t.makeXRCompatible(),p=e.getPixelRatio(),e.getSize(U),v&&"createProjectionLayer"in XRWebGLBinding.prototype){let be=null,Ge=null,Me=null;M.depth&&(Me=M.stencil?t.DEPTH24_STENCIL8:t.DEPTH_COMPONENT24,be=M.stencil?Qi:hi,Ge=M.stencil?ls:ii);let ze={colorFormat:t.RGBA8,depthFormat:Me,scaleFactor:s};m=this.getBinding(),u=m.createProjectionLayer(ze),r.updateRenderState({layers:[u]}),e.setPixelRatio(1),e.setSize(u.textureWidth,u.textureHeight,!1),w=new kn(u.textureWidth,u.textureHeight,{format:Fn,type:Un,depthTexture:new Gi(u.textureWidth,u.textureHeight,Ge,void 0,void 0,void 0,void 0,void 0,void 0,be),stencilBuffer:M.stencil,colorSpace:e.outputColorSpace,samples:M.antialias?4:0,resolveDepthBuffer:u.ignoreDepthValues===!1,resolveStencilBuffer:u.ignoreDepthValues===!1,storeMultisampledDepthBuffer:u.ignoreDepthValues===!1,storeMultisampledStencilBuffer:u.ignoreDepthValues===!1})}else{let be={antialias:M.antialias,alpha:!0,depth:M.depth,stencil:M.stencil,framebufferScaleFactor:s};g=new XRWebGLLayer(r,t,be),r.updateRenderState({baseLayer:g}),e.setPixelRatio(1),e.setSize(g.framebufferWidth,g.framebufferHeight,!1),w=new kn(g.framebufferWidth,g.framebufferHeight,{format:Fn,type:Un,colorSpace:e.outputColorSpace,stencilBuffer:M.stencil,resolveDepthBuffer:g.ignoreDepthValues===!1,resolveStencilBuffer:g.ignoreDepthValues===!1,storeMultisampledDepthBuffer:g.ignoreDepthValues===!1,storeMultisampledStencilBuffer:g.ignoreDepthValues===!1})}w.isXRRenderTarget=!0,this.setFoveation(l),c=null,o=await r.requestReferenceSpace(a),Oe.setContext(r),Oe.start(),i.isPresenting=!0,i.dispatchEvent({type:"sessionstart"})}},this.getEnvironmentBlendMode=function(){if(r!==null)return r.environmentBlendMode},this.getDepthTexture=function(){return f.getDepthTexture()};function ee(oe){for(let le=0;le<oe.removed.length;le++){let be=oe.removed[le],Ge=E.indexOf(be);Ge>=0&&(E[Ge]=null,T[Ge].disconnect(be))}for(let le=0;le<oe.added.length;le++){let be=oe.added[le],Ge=E.indexOf(be);if(Ge===-1){for(let ze=0;ze<T.length;ze++)if(ze>=E.length){E.push(be),Ge=ze;break}else if(E[ze]===null){E[ze]=be,Ge=ze;break}if(Ge===-1)break}let Me=T[Ge];Me&&Me.connect(be)}}let J=new G,ie=new G;function ce(oe,le,be){J.setFromMatrixPosition(le.matrixWorld),ie.setFromMatrixPosition(be.matrixWorld);let Ge=J.distanceTo(ie),Me=le.projectionMatrix.elements,ze=be.projectionMatrix.elements,Ut=Me[14]/(Me[10]-1),Je=Me[14]/(Me[10]+1),st=(Me[9]+1)/Me[5],gt=(Me[9]-1)/Me[5],tt=(Me[8]-1)/Me[0],Tt=(ze[8]+1)/ze[0],Ft=Ut*tt,Wt=Ut*Tt,Pt=Ge/(-tt+Tt),Ot=Pt*-tt;if(le.matrixWorld.decompose(oe.position,oe.quaternion,oe.scale),oe.translateX(Ot),oe.translateZ(Pt),oe.matrixWorld.compose(oe.position,oe.quaternion,oe.scale),oe.matrixWorldInverse.copy(oe.matrixWorld).invert(),Me[10]===-1)oe.projectionMatrix.copy(le.projectionMatrix),oe.projectionMatrixInverse.copy(le.projectionMatrixInverse);else{let W=Ut+Pt,tn=Je+Pt,ct=Ft-Ot,F=Wt+(Ge-Ot),b=st*Je/tn*W,Y=gt*Je/tn*W;oe.projectionMatrix.makePerspective(ct,F,b,Y,W,tn),oe.projectionMatrixInverse.copy(oe.projectionMatrix).invert()}}function we(oe,le){le===null?oe.matrixWorld.copy(oe.matrix):oe.matrixWorld.multiplyMatrices(le.matrixWorld,oe.matrix),oe.matrixWorldInverse.copy(oe.matrixWorld).invert()}this.updateCamera=function(oe){if(r===null)return;let le=oe.near,be=oe.far;f.texture!==null&&(f.depthNear>0&&(le=f.depthNear),f.depthFar>0&&(be=f.depthFar)),k.near=R.near=y.near=le,k.far=R.far=y.far=be,(D!==k.near||N!==k.far)&&(r.updateRenderState({depthNear:k.near,depthFar:k.far}),D=k.near,N=k.far),k.layers.mask=oe.layers.mask|6,y.layers.mask=k.layers.mask&-5,R.layers.mask=k.layers.mask&-3;let Ge=oe.parent,Me=k.cameras;we(k,Ge);for(let ze=0;ze<Me.length;ze++)we(Me[ze],Ge);Me.length===2?ce(k,y,R):k.projectionMatrix.copy(y.projectionMatrix),S===null&&oe.isPerspectiveCamera&&(S={camera:oe,fov:oe.fov,zoom:oe.zoom}),pe(oe,k,Ge)};function pe(oe,le,be){be===null?oe.matrix.copy(le.matrixWorld):(oe.matrix.copy(be.matrixWorld),oe.matrix.invert(),oe.matrix.multiply(le.matrixWorld)),oe.matrix.decompose(oe.position,oe.quaternion,oe.scale),oe.updateMatrixWorld(!0),oe.projectionMatrix.copy(le.projectionMatrix),oe.projectionMatrixInverse.copy(le.projectionMatrixInverse),oe.isPerspectiveCamera&&(oe.fov=Yr*2*Math.atan(1/oe.projectionMatrix.elements[5]),oe.zoom=1)}this.getCamera=function(){return k},this.getFoveation=function(){if(!(u===null&&g===null))return l},this.setFoveation=function(oe){l=oe,u!==null&&(u.fixedFoveation=oe),g!==null&&g.fixedFoveation!==void 0&&(g.fixedFoveation=oe)},this.hasDepthSensing=function(){return f.texture!==null},this.getDepthSensingMesh=function(){return f.getMesh(k)},this.getCameraTexture=function(oe){return h[oe]};let Pe=null;function Ve(oe,le){if(d=le.getViewerPose(c||o),_=le,d!==null){let be=d.views;g!==null&&(e.setRenderTargetFramebuffer(w,g.framebuffer),e.setRenderTarget(w));let Ge=!1;be.length!==k.cameras.length&&(k.cameras.length=0,Ge=!0);for(let Je=0;Je<be.length;Je++){let st=be[Je],gt=null;if(g!==null)gt=g.getViewport(st);else{let Tt=m.getViewSubImage(u,st);gt=Tt.viewport,Je===0&&(e.setRenderTargetTextures(w,Tt.colorTexture,Tt.depthStencilTexture),e.setRenderTarget(w))}let tt=I[Je];tt===void 0&&(tt=new gn,tt.layers.enable(Je),tt.viewport=new Bt,I[Je]=tt),tt.matrix.fromArray(st.transform.matrix),tt.matrix.decompose(tt.position,tt.quaternion,tt.scale),tt.projectionMatrix.fromArray(st.projectionMatrix),tt.projectionMatrixInverse.copy(tt.projectionMatrix).invert(),tt.viewport.set(gt.x,gt.y,gt.width,gt.height),Je===0&&(k.matrix.copy(tt.matrix),k.matrix.decompose(k.position,k.quaternion,k.scale)),Ge===!0&&k.cameras.push(tt)}let Me=r.enabledFeatures;if(Me&&Me.includes("depth-sensing")&&r.depthUsage=="gpu-optimized"&&v){m=i.getBinding();let Je=m.getDepthInformation(be[0]);Je&&Je.isValid&&Je.texture&&f.init(Je,r.renderState)}if(Me&&Me.includes("camera-access")&&v){e.state.unbindTexture(),m=i.getBinding();for(let Je=0;Je<be.length;Je++){let st=be[Je].camera;if(st){let gt=h[st];gt||(gt=new Gs,h[st]=gt);let tt=m.getCameraImage(st);gt.sourceTexture=tt}}}}for(let be=0;be<T.length;be++){let Ge=E[be],Me=T[be];Ge!==null&&Me!==void 0&&Me.update(Ge,le,c||o)}Pe&&Pe(oe,le),le.detectedPlanes&&i.dispatchEvent({type:"planesdetected",data:le}),_=null}let Oe=new Od;Oe.setAnimationLoop(Ve),this.setAnimationLoop=function(oe){Pe=oe},this.dispose=function(){}}},jv=new wt,Wd=new je;Wd.set(-1,0,0,0,1,0,0,0,1);function Qv(n,e){function t(f,h){f.matrixAutoUpdate===!0&&f.updateMatrix(),h.value.copy(f.matrix)}function i(f,h){h.color.getRGB(f.fogColor.value,Jc(n)),h.isFog?(f.fogNear.value=h.near,f.fogFar.value=h.far):h.isFogExp2&&(f.fogDensity.value=h.density)}function r(f,h,M,P,w){h.isNodeMaterial?h.uniformsNeedUpdate=!1:h.isMeshBasicMaterial?s(f,h):h.isMeshLambertMaterial?(s(f,h),h.envMap&&(f.envMapIntensity.value=h.envMapIntensity)):h.isMeshToonMaterial?(s(f,h),m(f,h)):h.isMeshPhongMaterial?(s(f,h),d(f,h),h.envMap&&(f.envMapIntensity.value=h.envMapIntensity)):h.isMeshStandardMaterial?(s(f,h),u(f,h),h.isMeshPhysicalMaterial&&g(f,h,w)):h.isMeshMatcapMaterial?(s(f,h),_(f,h)):h.isMeshDepthMaterial?s(f,h):h.isMeshDistanceMaterial?(s(f,h),v(f,h)):h.isMeshNormalMaterial?s(f,h):h.isLineBasicMaterial?(o(f,h),h.isLineDashedMaterial&&a(f,h)):h.isPointsMaterial?l(f,h,M,P):h.isSpriteMaterial?c(f,h):h.isShadowMaterial?(f.color.value.copy(h.color),f.opacity.value=h.opacity):h.isShaderMaterial&&(h.uniformsNeedUpdate=!1)}function s(f,h){f.opacity.value=h.opacity,h.color&&f.diffuse.value.copy(h.color),h.emissive&&f.emissive.value.copy(h.emissive).multiplyScalar(h.emissiveIntensity),h.map&&(f.map.value=h.map,t(h.map,f.mapTransform)),h.alphaMap&&(f.alphaMap.value=h.alphaMap,t(h.alphaMap,f.alphaMapTransform)),h.bumpMap&&(f.bumpMap.value=h.bumpMap,t(h.bumpMap,f.bumpMapTransform),f.bumpScale.value=h.bumpScale,h.side===fn&&(f.bumpScale.value*=-1)),h.normalMap&&(f.normalMap.value=h.normalMap,t(h.normalMap,f.normalMapTransform),f.normalScale.value.copy(h.normalScale),h.side===fn&&f.normalScale.value.negate()),h.displacementMap&&(f.displacementMap.value=h.displacementMap,t(h.displacementMap,f.displacementMapTransform),f.displacementScale.value=h.displacementScale,f.displacementBias.value=h.displacementBias),h.emissiveMap&&(f.emissiveMap.value=h.emissiveMap,t(h.emissiveMap,f.emissiveMapTransform)),h.specularMap&&(f.specularMap.value=h.specularMap,t(h.specularMap,f.specularMapTransform)),h.alphaTest>0&&(f.alphaTest.value=h.alphaTest);let M=e.get(h),P=M.envMap,w=M.envMapRotation;P&&(f.envMap.value=P,f.envMapRotation.value.setFromMatrix4(jv.makeRotationFromEuler(w)).transpose(),P.isCubeTexture&&P.isRenderTargetTexture===!1&&f.envMapRotation.value.premultiply(Wd),f.reflectivity.value=h.reflectivity,f.ior.value=h.ior,f.refractionRatio.value=h.refractionRatio),h.lightMap&&(f.lightMap.value=h.lightMap,f.lightMapIntensity.value=h.lightMapIntensity,t(h.lightMap,f.lightMapTransform)),h.aoMap&&(f.aoMap.value=h.aoMap,f.aoMapIntensity.value=h.aoMapIntensity,t(h.aoMap,f.aoMapTransform))}function o(f,h){f.diffuse.value.copy(h.color),f.opacity.value=h.opacity,h.map&&(f.map.value=h.map,t(h.map,f.mapTransform))}function a(f,h){f.dashSize.value=h.dashSize,f.totalSize.value=h.dashSize+h.gapSize,f.scale.value=h.scale}function l(f,h,M,P){f.diffuse.value.copy(h.color),f.opacity.value=h.opacity,f.size.value=h.size*M,f.scale.value=P*.5,h.map&&(f.map.value=h.map,t(h.map,f.uvTransform)),h.alphaMap&&(f.alphaMap.value=h.alphaMap,t(h.alphaMap,f.alphaMapTransform)),h.alphaTest>0&&(f.alphaTest.value=h.alphaTest)}function c(f,h){f.diffuse.value.copy(h.color),f.opacity.value=h.opacity,f.rotation.value=h.rotation,h.map&&(f.map.value=h.map,t(h.map,f.mapTransform)),h.alphaMap&&(f.alphaMap.value=h.alphaMap,t(h.alphaMap,f.alphaMapTransform)),h.alphaTest>0&&(f.alphaTest.value=h.alphaTest)}function d(f,h){f.specular.value.copy(h.specular),f.shininess.value=Math.max(h.shininess,1e-4)}function m(f,h){h.gradientMap&&(f.gradientMap.value=h.gradientMap)}function u(f,h){f.metalness.value=h.metalness,h.metalnessMap&&(f.metalnessMap.value=h.metalnessMap,t(h.metalnessMap,f.metalnessMapTransform)),f.roughness.value=h.roughness,h.roughnessMap&&(f.roughnessMap.value=h.roughnessMap,t(h.roughnessMap,f.roughnessMapTransform)),h.envMap&&(f.envMapIntensity.value=h.envMapIntensity)}function g(f,h,M){f.ior.value=h.ior,h.sheen>0&&(f.sheenColor.value.copy(h.sheenColor).multiplyScalar(h.sheen),f.sheenRoughness.value=h.sheenRoughness,h.sheenColorMap&&(f.sheenColorMap.value=h.sheenColorMap,t(h.sheenColorMap,f.sheenColorMapTransform)),h.sheenRoughnessMap&&(f.sheenRoughnessMap.value=h.sheenRoughnessMap,t(h.sheenRoughnessMap,f.sheenRoughnessMapTransform))),h.clearcoat>0&&(f.clearcoat.value=h.clearcoat,f.clearcoatRoughness.value=h.clearcoatRoughness,h.clearcoatMap&&(f.clearcoatMap.value=h.clearcoatMap,t(h.clearcoatMap,f.clearcoatMapTransform)),h.clearcoatRoughnessMap&&(f.clearcoatRoughnessMap.value=h.clearcoatRoughnessMap,t(h.clearcoatRoughnessMap,f.clearcoatRoughnessMapTransform)),h.clearcoatNormalMap&&(f.clearcoatNormalMap.value=h.clearcoatNormalMap,t(h.clearcoatNormalMap,f.clearcoatNormalMapTransform),f.clearcoatNormalScale.value.copy(h.clearcoatNormalScale),h.side===fn&&f.clearcoatNormalScale.value.negate())),h.dispersion>0&&(f.dispersion.value=h.dispersion),h.retroreflectivity>0&&(f.retroreflectivity.value=h.retroreflectivity),h.iridescence>0&&(f.iridescence.value=h.iridescence,f.iridescenceIOR.value=h.iridescenceIOR,f.iridescenceThicknessMinimum.value=h.iridescenceThicknessRange[0],f.iridescenceThicknessMaximum.value=h.iridescenceThicknessRange[1],h.iridescenceMap&&(f.iridescenceMap.value=h.iridescenceMap,t(h.iridescenceMap,f.iridescenceMapTransform)),h.iridescenceThicknessMap&&(f.iridescenceThicknessMap.value=h.iridescenceThicknessMap,t(h.iridescenceThicknessMap,f.iridescenceThicknessMapTransform))),h.transmission>0&&(f.transmission.value=h.transmission,f.transmissionSamplerMap.value=M.texture,f.transmissionSamplerSize.value.set(M.width,M.height),h.transmissionMap&&(f.transmissionMap.value=h.transmissionMap,t(h.transmissionMap,f.transmissionMapTransform)),f.thickness.value=h.thickness,h.thicknessMap&&(f.thicknessMap.value=h.thicknessMap,t(h.thicknessMap,f.thicknessMapTransform)),f.attenuationDistance.value=h.attenuationDistance,f.attenuationColor.value.copy(h.attenuationColor)),h.anisotropy>0&&(f.anisotropyVector.value.set(h.anisotropy*Math.cos(h.anisotropyRotation),h.anisotropy*Math.sin(h.anisotropyRotation)),h.anisotropyMap&&(f.anisotropyMap.value=h.anisotropyMap,t(h.anisotropyMap,f.anisotropyMapTransform))),f.specularIntensity.value=h.specularIntensity,f.specularColor.value.copy(h.specularColor),h.specularColorMap&&(f.specularColorMap.value=h.specularColorMap,t(h.specularColorMap,f.specularColorMapTransform)),h.specularIntensityMap&&(f.specularIntensityMap.value=h.specularIntensityMap,t(h.specularIntensityMap,f.specularIntensityMapTransform))}function _(f,h){h.matcap&&(f.matcap.value=h.matcap)}function v(f,h){let M=e.get(h).light;f.referencePosition.value.setFromMatrixPosition(M.matrixWorld),f.nearDistance.value=M.shadow.camera.near,f.farDistance.value=M.shadow.camera.far}return{refreshFogUniforms:i,refreshMaterialUniforms:r}}function e_(n,e,t,i){let r={},s={},o=[],a=n.getParameter(n.MAX_UNIFORM_BUFFER_BINDINGS);function l(w,T){let E=T.program;i.uniformBlockBinding(w,E)}function c(w,T){let E=r[w.id];E===void 0&&(f(w),E=d(w),r[w.id]=E,w.addEventListener("dispose",M));let U=T.program;i.updateUBOMapping(w,U);let p=e.render.frame;s[w.id]!==p&&(u(w),s[w.id]=p)}function d(w){let T=m();w.__bindingPointIndex=T;let E=n.createBuffer(),U=w.__size,p=w.usage;return n.bindBuffer(n.UNIFORM_BUFFER,E),n.bufferData(n.UNIFORM_BUFFER,U,p),n.bindBuffer(n.UNIFORM_BUFFER,null),n.bindBufferBase(n.UNIFORM_BUFFER,T,E),E}function m(){for(let w=0;w<a;w++)if(o.indexOf(w)===-1)return o.push(w),w;return Ze("WebGLRenderer: Maximum number of simultaneously usable uniforms groups reached."),0}function u(w){let T=r[w.id],E=w.uniforms,U=w.__cache;n.bindBuffer(n.UNIFORM_BUFFER,T);for(let p=0,S=E.length;p<S;p++){let y=E[p];if(Array.isArray(y))for(let R=0,I=y.length;R<I;R++)g(y[R],p,R,U);else g(y,p,0,U)}n.bindBuffer(n.UNIFORM_BUFFER,null)}function g(w,T,E,U){if(v(w,T,E,U)===!0){let p=w.__offset,S=w.value;if(Array.isArray(S)){let y=0;for(let R=0;R<S.length;R++){let I=S[R],k=h(I);_(I,w.__data,y),typeof I!="number"&&typeof I!="boolean"&&!I.isMatrix3&&!ArrayBuffer.isView(I)&&(y+=k.storage/Float32Array.BYTES_PER_ELEMENT)}}else _(S,w.__data,0);n.bufferSubData(n.UNIFORM_BUFFER,p,w.__data)}}function _(w,T,E){typeof w=="number"||typeof w=="boolean"?T[0]=w:w.isMatrix3?(T[0]=w.elements[0],T[1]=w.elements[1],T[2]=w.elements[2],T[3]=0,T[4]=w.elements[3],T[5]=w.elements[4],T[6]=w.elements[5],T[7]=0,T[8]=w.elements[6],T[9]=w.elements[7],T[10]=w.elements[8],T[11]=0):ArrayBuffer.isView(w)?T.set(new w.constructor(w.buffer,w.byteOffset,T.length)):w.toArray(T,E)}function v(w,T,E,U){let p=w.value,S=T+"_"+E;if(U[S]===void 0)return typeof p=="number"||typeof p=="boolean"?U[S]=p:ArrayBuffer.isView(p)?U[S]=p.slice():U[S]=p.clone(),!0;{let y=U[S];if(typeof p=="number"||typeof p=="boolean"){if(y!==p)return U[S]=p,!0}else{if(ArrayBuffer.isView(p))return!0;if(y.equals(p)===!1)return y.copy(p),!0}}return!1}function f(w){let T=w.uniforms,E=0,U=16;for(let S=0,y=T.length;S<y;S++){let R=Array.isArray(T[S])?T[S]:[T[S]];for(let I=0,k=R.length;I<k;I++){let D=R[I],N=Array.isArray(D.value)?D.value:[D.value];for(let H=0,$=N.length;H<$;H++){let ee=N[H],J=h(ee),ie=E%U,ce=ie%J.boundary,we=ie+ce;E+=ce,we!==0&&U-we<J.storage&&(E+=U-we),D.__data=new Float32Array(J.storage/Float32Array.BYTES_PER_ELEMENT),D.__offset=E,E+=J.storage}}}let p=E%U;return p>0&&(E+=U-p),w.__size=E,w.__cache={},this}function h(w){let T={boundary:0,storage:0};return typeof w=="number"||typeof w=="boolean"?(T.boundary=4,T.storage=4):w.isVector2?(T.boundary=8,T.storage=8):w.isVector3||w.isColor?(T.boundary=16,T.storage=12):w.isVector4?(T.boundary=16,T.storage=16):w.isMatrix3?(T.boundary=48,T.storage=48):w.isMatrix4?(T.boundary=64,T.storage=64):w.isTexture?$e("WebGLRenderer: Texture samplers can not be part of an uniforms group."):ArrayBuffer.isView(w)?(T.boundary=16,T.storage=w.byteLength):$e("WebGLRenderer: Unsupported uniform value type.",w),T}function M(w){let T=w.target;T.removeEventListener("dispose",M);let E=o.indexOf(T.__bindingPointIndex);o.splice(E,1),n.deleteBuffer(r[T.id]),delete r[T.id],delete s[T.id]}function P(){for(let w in r)n.deleteBuffer(r[w]);o=[],r={},s={}}return{bind:l,update:c,dispose:P}}var t_=new Uint16Array([12469,15057,12620,14925,13266,14620,13807,14376,14323,13990,14545,13625,14713,13328,14840,12882,14931,12528,14996,12233,15039,11829,15066,11525,15080,11295,15085,10976,15082,10705,15073,10495,13880,14564,13898,14542,13977,14430,14158,14124,14393,13732,14556,13410,14702,12996,14814,12596,14891,12291,14937,11834,14957,11489,14958,11194,14943,10803,14921,10506,14893,10278,14858,9960,14484,14039,14487,14025,14499,13941,14524,13740,14574,13468,14654,13106,14743,12678,14818,12344,14867,11893,14889,11509,14893,11180,14881,10751,14852,10428,14812,10128,14765,9754,14712,9466,14764,13480,14764,13475,14766,13440,14766,13347,14769,13070,14786,12713,14816,12387,14844,11957,14860,11549,14868,11215,14855,10751,14825,10403,14782,10044,14729,9651,14666,9352,14599,9029,14967,12835,14966,12831,14963,12804,14954,12723,14936,12564,14917,12347,14900,11958,14886,11569,14878,11247,14859,10765,14828,10401,14784,10011,14727,9600,14660,9289,14586,8893,14508,8533,15111,12234,15110,12234,15104,12216,15092,12156,15067,12010,15028,11776,14981,11500,14942,11205,14902,10752,14861,10393,14812,9991,14752,9570,14682,9252,14603,8808,14519,8445,14431,8145,15209,11449,15208,11451,15202,11451,15190,11438,15163,11384,15117,11274,15055,10979,14994,10648,14932,10343,14871,9936,14803,9532,14729,9218,14645,8742,14556,8381,14461,8020,14365,7603,15273,10603,15272,10607,15267,10619,15256,10631,15231,10614,15182,10535,15118,10389,15042,10167,14963,9787,14883,9447,14800,9115,14710,8665,14615,8318,14514,7911,14411,7507,14279,7198,15314,9675,15313,9683,15309,9712,15298,9759,15277,9797,15229,9773,15166,9668,15084,9487,14995,9274,14898,8910,14800,8539,14697,8234,14590,7790,14479,7409,14367,7067,14178,6621,15337,8619,15337,8631,15333,8677,15325,8769,15305,8871,15264,8940,15202,8909,15119,8775,15022,8565,14916,8328,14804,8009,14688,7614,14569,7287,14448,6888,14321,6483,14088,6171,15350,7402,15350,7419,15347,7480,15340,7613,15322,7804,15287,7973,15229,8057,15148,8012,15046,7846,14933,7611,14810,7357,14682,7069,14552,6656,14421,6316,14251,5948,14007,5528,15356,5942,15356,5977,15353,6119,15348,6294,15332,6551,15302,6824,15249,7044,15171,7122,15070,7050,14949,6861,14818,6611,14679,6349,14538,6067,14398,5651,14189,5311,13935,4958,15359,4123,15359,4153,15356,4296,15353,4646,15338,5160,15311,5508,15263,5829,15188,6042,15088,6094,14966,6001,14826,5796,14678,5543,14527,5287,14377,4985,14133,4586,13869,4257,15360,1563,15360,1642,15358,2076,15354,2636,15341,3350,15317,4019,15273,4429,15203,4732,15105,4911,14981,4932,14836,4818,14679,4621,14517,4386,14359,4156,14083,3795,13808,3437,15360,122,15360,137,15358,285,15355,636,15344,1274,15322,2177,15281,2765,15215,3223,15120,3451,14995,3569,14846,3567,14681,3466,14511,3305,14344,3121,14037,2800,13753,2467,15360,0,15360,1,15359,21,15355,89,15346,253,15325,479,15287,796,15225,1148,15133,1492,15008,1749,14856,1882,14685,1886,14506,1783,14324,1608,13996,1398,13702,1183]),pi=null;function n_(){return pi===null&&(pi=new fr(t_,16,16,er,ri),pi.name="DFG_LUT",pi.minFilter=xn,pi.magFilter=xn,pi.wrapS=ui,pi.wrapT=ui,pi.generateMipmaps=!1,pi.needsUpdate=!0),pi}var Ml=class{constructor(e={}){let{canvas:t=ud(),context:i=null,depth:r=!0,stencil:s=!1,alpha:o=!1,antialias:a=!1,premultipliedAlpha:l=!0,preserveDrawingBuffer:c=!1,powerPreference:d="default",failIfMajorPerformanceCaveat:m=!1,reversedDepthBuffer:u=!1,outputBufferType:g=Un}=e;this.isWebGLRenderer=!0;let _;if(i!==null){if(typeof WebGLRenderingContext<"u"&&i instanceof WebGLRenderingContext)throw new Error("THREE.WebGLRenderer: WebGL 1 is not supported since r163.");_=i.getContextAttributes().alpha}else _=o;let v=g,f=new Set([Fa,Ua,Na]),h=new Set([Un,ii,as,ls,La,ka]),M=new Uint32Array(4),P=new Int32Array(4),w=new G,T=null,E=null,U=[],p=[],S=null;this.domElement=t,this.debug={checkShaderErrors:!0,diagnostics:{keywords:!1},onShaderError:null},this.autoClear=!0,this.autoClearColor=!0,this.autoClearDepth=!0,this.autoClearStencil=!0,this.sortObjects=!0,this.clippingPlanes=[],this.localClippingEnabled=!1,this.toneMapping=ni,this.toneMappingExposure=1,this.transmissionResolutionScale=1;let y=this,R=!1,I=null,k=null,D=null,N=null;this._outputColorSpace=kt;let H=0,$=0,ee=null,J=-1,ie=null,ce=new Bt,we=new Bt,pe=null,Pe=new Ne(0),Ve=0,Oe=t.width,oe=t.height,le=1,be=null,Ge=null,Me=new Bt(0,0,Oe,oe),ze=new Bt(0,0,Oe,oe),Ut=!1,Je=new jr,st=!1,gt=!1,tt=new wt,Tt=new G,Ft=new Bt,Wt={background:null,fog:null,environment:null,overrideMaterial:null,isScene:!0},Pt=!1;function Ot(){return ee===null?le:1}let W=i;function tn(A,V){return t.getContext(A,V)}let ct,F,b,Y,te,ae,ve,ye,C,L,O,z,X,Z,re,fe,me,B,ue,se,ge,_e,he;try{let A={alpha:!0,depth:r,stencil:s,antialias:a,premultipliedAlpha:l,preserveDrawingBuffer:c,powerPreference:d,failIfMajorPerformanceCaveat:m};if("setAttribute"in t&&t.setAttribute("data-engine",`three.js r${"186"}`),t.addEventListener("webglcontextlost",_t,!1),t.addEventListener("webglcontextrestored",ot,!1),t.addEventListener("webglcontextcreationerror",Rt,!1),W===null){let V="webgl2";if(W=tn(V,A),W===null)throw tn(V)?new Error("THREE.WebGLRenderer: Error creating WebGL context with your selected attributes."):new Error("THREE.WebGLRenderer: Error creating WebGL context.")}Ce()}catch(A){throw t.removeEventListener("webglcontextlost",_t,!1),t.removeEventListener("webglcontextrestored",ot,!1),t.removeEventListener("webglcontextcreationerror",Rt,!1),Ze("WebGLRenderer: "+A.message),A}function Ce(){ct=new cx(W),ct.init(),ge=new Zv(W,ct),F=new Qg(W,ct,e,ge),b=new $v(W,ct),F.reversedDepthBuffer&&u&&b.buffers.depth.setReversed(!0),k=W.createFramebuffer(),D=W.createFramebuffer(),N=W.createFramebuffer(),Y=new dx(W),te=new kv,ae=new Yv(W,ct,b,te,F,ge,Y),ve=new lx(y),ye=new pm(W),_e=new Jg(W,ye),C=new ux(W,ye,Y,_e),L=new px(W,C,ye,_e,Y),B=new fx(W,F,ae),re=new ex(te),O=new Lv(y,ve,ct,F,_e,re),z=new Qv(y,te),X=new Nv,Z=new Hv(ct),me=new Kg(y,ve,b,L,_,l),fe=new qv(y,L,F),he=new e_(W,Y,F,b),ue=new jg(W,ct,Y),se=new hx(W,ct,Y),Y.programs=O.programs,y.capabilities=F,y.extensions=ct,y.properties=te,y.renderLists=X,y.shadowMap=fe,y.state=b,y.info=Y}v!==Un&&(S=new gx(v,t.width,t.height,a,r,s));let Le=new mu(y,W);this.xr=Le,this.getContext=function(){return W},this.getContextAttributes=function(){return W.getContextAttributes()},this.forceContextLoss=function(){let A=ct.get("WEBGL_lose_context");A&&A.loseContext()},this.forceContextRestore=function(){let A=ct.get("WEBGL_lose_context");A&&A.restoreContext()},this.getPixelRatio=function(){return le},this.setPixelRatio=function(A){A!==void 0&&(le=A,this.setSize(Oe,oe,!1))},this.getSize=function(A){return A.set(Oe,oe)},this.setSize=function(A,V,ne=!0){if(Le.isPresenting){$e("WebGLRenderer: Can't change size while VR device is presenting.");return}Oe=A,oe=V,t.width=Math.floor(A*le),t.height=Math.floor(V*le),ne===!0&&(t.style.width=A+"px",t.style.height=V+"px"),S!==null&&S.setSize(t.width,t.height),this.setViewport(0,0,A,V)},this.getDrawingBufferSize=function(A){return A.set(Oe*le,oe*le).floor()},this.setDrawingBufferSize=function(A,V,ne){Oe=A,oe=V,le=ne,t.width=Math.floor(A*ne),t.height=Math.floor(V*ne),this.setViewport(0,0,A,V)},this.setEffects=function(A){if(v===Un){Ze("WebGLRenderer: setEffects() requires outputBufferType set to HalfFloatType or FloatType.");return}if(A){for(let V=0;V<A.length;V++)if(A[V].isOutputPass===!0){$e("WebGLRenderer: OutputPass is not needed in setEffects(). Tone mapping and color space conversion are applied automatically.");break}}S.setEffects(A||[])},this.getCurrentViewport=function(A){return A.copy(ce)},this.getViewport=function(A){return A.copy(Me)},this.setViewport=function(A,V,ne,Q){A.isVector4?Me.set(A.x,A.y,A.z,A.w):Me.set(A,V,ne,Q),b.viewport(ce.copy(Me).multiplyScalar(le).round())},this.getScissor=function(A){return A.copy(ze)},this.setScissor=function(A,V,ne,Q){A.isVector4?ze.set(A.x,A.y,A.z,A.w):ze.set(A,V,ne,Q),b.scissor(we.copy(ze).multiplyScalar(le).round())},this.getScissorTest=function(){return Ut},this.setScissorTest=function(A){b.setScissorTest(Ut=A)},this.setOpaqueSort=function(A){be=A},this.setTransparentSort=function(A){Ge=A},this.getClearColor=function(A){return A.copy(me.getClearColor())},this.setClearColor=function(){me.setClearColor(...arguments)},this.getClearAlpha=function(){return me.getClearAlpha()},this.setClearAlpha=function(){me.setClearAlpha(...arguments)},this.clear=function(A=!0,V=!0,ne=!0){let Q=0;if(A){let j=!1;if(ee!==null){let Re=ee.texture.format;j=f.has(Re)}if(j){let Re=ee.texture.type,ke=h.has(Re),Ee=me.getClearColor(),Ae=me.getClearAlpha(),Ue=Ee.r,et=Ee.g,nt=Ee.b;ke?(M[0]=Ue,M[1]=et,M[2]=nt,M[3]=Ae,W.clearBufferuiv(W.COLOR,0,M)):(P[0]=Ue,P[1]=et,P[2]=nt,P[3]=Ae,W.clearBufferiv(W.COLOR,0,P))}else Q|=W.COLOR_BUFFER_BIT}V&&(Q|=W.DEPTH_BUFFER_BIT,this.state.buffers.depth.setMask(!0)),ne&&(Q|=W.STENCIL_BUFFER_BIT,this.state.buffers.stencil.setMask(4294967295)),Q!==0&&W.clear(Q)},this.clearColor=function(){this.clear(!0,!1,!1)},this.clearDepth=function(){this.clear(!1,!0,!1)},this.clearStencil=function(){this.clear(!1,!1,!0)},this.setNodesHandler=function(A){A.setRenderer(this),I=A},this.dispose=function(){t.removeEventListener("webglcontextlost",_t,!1),t.removeEventListener("webglcontextrestored",ot,!1),t.removeEventListener("webglcontextcreationerror",Rt,!1),me.dispose(),X.dispose(),Z.dispose(),te.dispose(),ve.dispose(),L.dispose(),_e.dispose(),he.dispose(),O.dispose(),Le.dispose(),Le.removeEventListener("sessionstart",q),Le.removeEventListener("sessionend",de),xe.stop()};function _t(A){A.preventDefault(),Yc("WebGLRenderer: Context Lost."),R=!0}function ot(){Yc("WebGLRenderer: Context Restored."),R=!1;let A=Y.autoReset,V=fe.enabled,ne=fe.autoUpdate,Q=fe.needsUpdate,j=fe.type;Ce(),Y.autoReset=A,fe.enabled=V,fe.autoUpdate=ne,fe.needsUpdate=Q,fe.type=j}function Rt(A){Ze("WebGLRenderer: A WebGL context could not be created. Reason: ",A.statusMessage)}function on(A){let V=A.target;V.removeEventListener("dispose",on),Fl(V)}function Fl(A){Ol(A),te.remove(A)}function Ol(A){let V=te.get(A).programs;V!==void 0&&(V.forEach(function(ne){O.releaseProgram(ne)}),A.isShaderMaterial&&O.releaseShaderCache(A))}this.renderBufferDirect=function(A,V,ne,Q,j,Re){V===null&&(V=Wt);let ke=j.isMesh&&j.matrixWorld.determinantAffine()<0,Ee=ir(A,V,ne,Q,j);b.setMaterial(Q,ke);let Ae=ne.index,Ue=1;if(Q.wireframe===!0){if(Ae=C.getWireframeAttribute(ne),Ae===void 0)return;Ue=2}let et=ne.drawRange,nt=ne.attributes.position,Fe=et.start*Ue,xt=(et.start+et.count)*Ue;Re!==null&&(Fe=Math.max(Fe,Re.start*Ue),xt=Math.min(xt,(Re.start+Re.count)*Ue)),Ae!==null?(Fe=Math.max(Fe,0),xt=Math.min(xt,Ae.count)):nt!=null&&(Fe=Math.max(Fe,0),xt=Math.min(xt,nt.count));let nn=xt-Fe;if(nn<0||nn===1/0)return;_e.setup(j,Q,Ee,ne,Ae);let Dt,It=ue;if(Ae!==null&&(Dt=ye.get(Ae),It=se,It.setIndex(Dt)),j.isMesh)Q.wireframe===!0?(b.setLineWidth(Q.wireframeLinewidth*Ot()),It.setMode(W.LINES)):It.setMode(W.TRIANGLES);else if(j.isLine){let yn=Q.linewidth;yn===void 0&&(yn=1),b.setLineWidth(yn*Ot()),j.isLineSegments?It.setMode(W.LINES):j.isLineLoop?It.setMode(W.LINE_LOOP):It.setMode(W.LINE_STRIP)}else j.isPoints?It.setMode(W.POINTS):j.isSprite&&It.setMode(W.TRIANGLES);if(j.isBatchedMesh)if(ct.get("WEBGL_multi_draw"))It.renderMultiDraw(j._multiDrawStarts,j._multiDrawCounts,j._multiDrawCount);else{let yn=j._multiDrawStarts,De=j._multiDrawCounts,Rn=j._multiDrawCount,vt=Ae?ye.get(Ae).bytesPerElement:1,Wn=te.get(Q).currentProgram.getUniforms();for(let oi=0;oi<Rn;oi++)Wn.setValue(W,"_gl_DrawID",oi),It.render(yn[oi]/vt,De[oi])}else if(j.isInstancedMesh)It.renderInstances(Fe,nn,j.count);else if(ne.isInstancedBufferGeometry){let yn=ne._maxInstanceCount!==void 0?ne._maxInstanceCount:1/0,De=Math.min(ne.instanceCount,yn);It.renderInstances(Fe,nn,De)}else It.render(Fe,nn)};function Eo(A,V,ne,Q){I!==null&&A.isNodeMaterial&&I.setObject(Q,A),st===!0&&re.setState(A,ne,!1),A.transparent===!0&&A.side===Nn&&A.forceSinglePass===!1?(A.side=fn,A.needsUpdate=!0,Ht(A,V,Q),A.side=Ki,A.needsUpdate=!0,Ht(A,V,Q),A.side=Nn):Ht(A,V,Q)}this.compile=function(A,V,ne=null){ne===null&&(ne=A),I!==null&&I.renderStart(A,V,ne),E=Z.get(ne),E.init(V),p.push(E),ne.traverseVisible(function(j){j.isLight&&j.layers.test(V.layers)&&(E.pushLight(j),j.castShadow&&E.pushShadow(j))}),A!==ne&&A.traverseVisible(function(j){j.isLight&&j.layers.test(V.layers)&&(E.pushLight(j),j.castShadow&&E.pushShadow(j))}),E.setupLights(),I!==null&&I.updateLights(E.state.lightsArray),gt=this.localClippingEnabled,st=re.init(this.clippingPlanes,gt),st===!0&&re.setGlobalState(this.clippingPlanes,V),I!==null&&fe.render(E.state.shadowsArray,ne,V);let Q=new Set;return A.traverse(function(j){if(!(j.isMesh||j.isPoints||j.isLine||j.isSprite))return;let Re=j.material;if(Re)if(Array.isArray(Re))for(let ke=0;ke<Re.length;ke++){let Ee=Re[ke];Eo(Ee,ne,V,j),Q.add(Ee)}else Eo(Re,ne,V,j),Q.add(Re)}),E=p.pop(),I!==null&&I.renderEnd(),Q},this.compileAsync=function(A,V,ne=null){let Q=this.compile(A,V,ne);return new Promise(j=>{function Re(){if(Q.forEach(function(ke){let Ae=te.get(ke).currentProgram;(Ae===void 0||Ae.isReady())&&Q.delete(ke)}),Q.size===0){j(A);return}setTimeout(Re,10)}ct.get("KHR_parallel_shader_compile")!==null?Re():setTimeout(Re,10)})};let gs=null;function Bl(A){gs&&gs(A)}function q(){xe.stop()}function de(){xe.start()}let xe=new Od;xe.setAnimationLoop(Bl),typeof self<"u"&&xe.setContext(self),this.setAnimationLoop=function(A){gs=A,Le.setAnimationLoop(A),A===null?xe.stop():xe.start()},Le.addEventListener("sessionstart",q),Le.addEventListener("sessionend",de),this.render=function(A,V){if(V!==void 0&&V.isCamera!==!0){Ze("WebGLRenderer.render: camera is not an instance of THREE.Camera.");return}if(R===!0)return;I!==null&&I.renderStart(A,V);let ne=Le.enabled===!0&&Le.isPresenting===!0,Q=S!==null&&(ee===null||ne)&&S.begin(y,ee);if(A.matrixWorldAutoUpdate===!0&&A.updateMatrixWorld(),V.parent===null&&V.matrixWorldAutoUpdate===!0&&V.updateMatrixWorld(),Le.enabled===!0&&Le.isPresenting===!0&&(S===null||S.isCompositing()===!1)&&(Le.cameraAutoUpdate===!0&&Le.updateCamera(V),V=Le.getCamera()),A.isScene===!0&&A.onBeforeRender(y,A,V,ee),E=Z.get(A,p.length),E.init(V),E.state.textureUnits=ae.getTextureUnits(),p.push(E),tt.multiplyMatrices(V.projectionMatrix,V.matrixWorldInverse),Je.setFromProjectionMatrix(tt,ei,V.reversedDepth),gt=this.localClippingEnabled,st=re.init(this.clippingPlanes,gt),T=X.get(A,U.length),T.init(),U.push(T),Le.enabled===!0&&Le.isPresenting===!0){let ke=y.xr.getDepthSensingMesh();ke!==null&&K(ke,V,-1/0,y.sortObjects)}K(A,V,0,y.sortObjects),T.finish(),I!==null&&I.updateLights(E.state.lightsArray),y.sortObjects===!0&&T.sort(be,Ge),Pt=Le.enabled===!1||Le.isPresenting===!1||Le.hasDepthSensing()===!1,Pt&&me.addToRenderList(T,A),this.info.render.frame++,this.info.autoReset===!0&&this.info.reset(),st===!0&&re.beginShadows();let j=E.state.shadowsArray;if(fe.render(j,A,V),st===!0&&re.endShadows(),(Q&&S.hasRenderPass())===!1){let ke=T.opaque,Ee=T.transmissive;if(E.setupLights(),V.isArrayCamera){let Ae=V.cameras;if(Ee.length>0)for(let Ue=0,et=Ae.length;Ue<et;Ue++){let nt=Ae[Ue];Se(ke,Ee,A,nt)}Pt&&me.render(A);for(let Ue=0,et=Ae.length;Ue<et;Ue++){let nt=Ae[Ue];Ie(T,A,nt,nt.viewport)}}else Ee.length>0&&Se(ke,Ee,A,V),Pt&&me.render(A),Ie(T,A,V)}ee!==null&&$===0&&(ae.updateMultisampleRenderTarget(ee),ae.updateRenderTargetMipmap(ee)),Q&&S.end(y),A.isScene===!0&&A.onAfterRender(y,A,V),_e.resetDefaultState(),J=-1,ie=null,p.pop(),p.length>0?(E=p[p.length-1],ae.setTextureUnits(E.state.textureUnits),st===!0&&re.setGlobalState(y.clippingPlanes,E.state.camera)):E=null,U.pop(),U.length>0?T=U[U.length-1]:T=null,I!==null&&I.renderEnd()};function K(A,V,ne,Q){if(A.visible===!1)return;if(A.layers.test(V.layers)){if(A.isGroup)ne=A.renderOrder;else if(A.isLOD)A.autoUpdate===!0&&A.update(V);else if(A.isLightProbeGrid)E.pushLightProbeGrid(A);else if(A.isLight)E.pushLight(A),A.castShadow&&E.pushShadow(A);else if(A.isSprite){if(!A.frustumCulled||A.intersectsFrustum(Je)){Q&&Ft.setFromMatrixPosition(A.matrixWorld).applyMatrix4(tt);let ke=L.update(A),Ee=A.material;Ee.visible&&T.push(A,ke,Ee,ne,Ft.z,null,V)}}else if((A.isMesh||A.isLine||A.isPoints)&&(!A.frustumCulled||A.intersectsFrustum(Je))){let ke=L.update(A),Ee=A.material;if(Q&&(A.boundingSphere!==void 0?(A.boundingSphere===null&&A.computeBoundingSphere(),Ft.copy(A.boundingSphere.center)):(ke.boundingSphere===null&&ke.computeBoundingSphere(),Ft.copy(ke.boundingSphere.center)),Ft.applyMatrix4(A.matrixWorld).applyMatrix4(tt)),Array.isArray(Ee)){let Ae=ke.groups;for(let Ue=0,et=Ae.length;Ue<et;Ue++){let nt=Ae[Ue],Fe=Ee[nt.materialIndex];Fe&&Fe.visible&&T.push(A,ke,Fe,ne,Ft.z,nt,V)}}else Ee.visible&&T.push(A,ke,Ee,ne,Ft.z,null,V)}}let Re=A.children;for(let ke=0,Ee=Re.length;ke<Ee;ke++)K(Re[ke],V,ne,Q)}function Ie(A,V,ne,Q){let{opaque:j,transmissive:Re,transparent:ke}=A;E.setupLightsView(ne),st===!0&&re.setGlobalState(y.clippingPlanes,ne),Q&&b.viewport(ce.copy(Q)),j.length>0&&Ke(j,V,ne),Re.length>0&&Ke(Re,V,ne),ke.length>0&&Ke(ke,V,ne),b.buffers.depth.setTest(!0),b.buffers.depth.setMask(!0),b.buffers.color.setMask(!0),b.setPolygonOffset(!1)}function Se(A,V,ne,Q){if((ne.isScene===!0?ne.overrideMaterial:null)!==null)return;if(E.state.transmissionRenderTarget[Q.id]===void 0){let Fe=ct.has("EXT_color_buffer_half_float")||ct.has("EXT_color_buffer_float");E.state.transmissionRenderTarget[Q.id]=new kn(1,1,{generateMipmaps:!0,type:Fe?ri:Un,minFilter:ji,samples:Math.max(4,F.samples),stencilBuffer:s,resolveDepthBuffer:!1,resolveStencilBuffer:!1,storeMultisampledDepthBuffer:!1,storeMultisampledStencilBuffer:!1,colorSpace:ut.workingColorSpace})}let Re=E.state.transmissionRenderTarget[Q.id],ke=Q.viewport||ce;Re.setSize(ke.z*y.transmissionResolutionScale,ke.w*y.transmissionResolutionScale);let Ee=y.getRenderTarget(),Ae=y.getActiveCubeFace(),Ue=y.getActiveMipmapLevel();y.setRenderTarget(Re),y.getClearColor(Pe),Ve=y.getClearAlpha(),Ve<1&&y.setClearColor(16777215,.5),y.clear(),Pt&&me.render(ne);let et=y.toneMapping;y.toneMapping=ni;let nt=Q.viewport;if(Q.viewport!==void 0&&(Q.viewport=void 0),E.setupLightsView(Q),st===!0&&re.setGlobalState(y.clippingPlanes,Q),Ke(A,ne,Q),ae.updateMultisampleRenderTarget(Re),ae.updateRenderTargetMipmap(Re),ct.has("WEBGL_multisampled_render_to_texture")===!1){let Fe=!1;for(let xt=0,nn=V.length;xt<nn;xt++){let Dt=V[xt],{object:It,geometry:yn,material:De,group:Rn}=Dt;if(De.side===Nn&&It.layers.test(Q.layers)){let vt=De.side;De.side=fn,De.needsUpdate=!0,Xe(It,ne,Q,yn,De,Rn),De.side=vt,De.needsUpdate=!0,Fe=!0}}Fe===!0&&(ae.updateMultisampleRenderTarget(Re),ae.updateRenderTargetMipmap(Re))}y.setRenderTarget(Ee,Ae,Ue),y.setClearColor(Pe,Ve),nt!==void 0&&(Q.viewport=nt),y.toneMapping=et}function Ke(A,V,ne){let Q=V.isScene===!0?V.overrideMaterial:null;for(let j=0,Re=A.length;j<Re;j++){let ke=A[j],{object:Ee,geometry:Ae,group:Ue}=ke,et=ke.material;et.allowOverride===!0&&Q!==null&&(et=Q),Ee.layers.test(ne.layers)&&Xe(Ee,V,ne,Ae,et,Ue)}}function Xe(A,V,ne,Q,j,Re){I!==null&&j.isNodeMaterial&&I.setObject(A,j),A.onBeforeRender(y,V,ne,Q,j,Re),A.modelViewMatrix.multiplyMatrices(ne.matrixWorldInverse,A.matrixWorld),A.normalMatrix.getNormalMatrix(A.modelViewMatrix),j.onBeforeRender(y,V,ne,Q,A,Re),j.transparent===!0&&j.side===Nn&&j.forceSinglePass===!1?(j.side=fn,j.needsUpdate=!0,y.renderBufferDirect(ne,V,Q,j,A,Re),j.side=Ki,j.needsUpdate=!0,y.renderBufferDirect(ne,V,Q,j,A,Re),j.side=Nn):y.renderBufferDirect(ne,V,Q,j,A,Re),A.onAfterRender(y,V,ne,Q,j,Re)}function Ht(A,V,ne){V.isScene!==!0&&(V=Wt);let Q=te.get(A),j=E.state.lights,Re=E.state.shadowsArray,ke=j.state.version,Ee=O.getParameters(A,j.state,Re,V,ne,E.state.lightProbeGridArray),Ae=O.getProgramCacheKey(Ee),Ue=Q.programs;Q.environment=A.isMeshStandardMaterial||A.isMeshLambertMaterial||A.isMeshPhongMaterial?V.environment:null,Q.fog=V.fog;let et=A.isMeshStandardMaterial||A.isMeshLambertMaterial&&!A.envMap||A.isMeshPhongMaterial&&!A.envMap;Q.envMap=ve.get(A.envMap||Q.environment,et),Q.envMapRotation=Q.environment!==null&&A.envMap===null?V.environmentRotation:A.envMapRotation,Ue===void 0&&(A.addEventListener("dispose",on),Ue=new Map,Q.programs=Ue);let nt=Ue.get(Ae);if(nt!==void 0){if(Q.currentProgram===nt&&Q.lightsStateVersion===ke)return _n(A,Ee),nt}else Ee.uniforms=O.getUniforms(A),I!==null&&A.isNodeMaterial&&I.build(A,ne,Ee),A.onBeforeCompile(Ee,y),nt=O.acquireProgram(Ee,Ae),Ue.set(Ae,nt),Q.uniforms=Ee.uniforms;let Fe=Q.uniforms;return(!A.isShaderMaterial&&!A.isRawShaderMaterial||A.clipping===!0)&&(Fe.clippingPlanes=re.uniform),_n(A,Ee),Q.needsLights=To(A),Q.lightsStateVersion=ke,Q.needsLights&&(Fe.ambientLightColor.value=j.state.ambient,Fe.lightProbe.value=j.state.probe,Fe.sunLights.value=j.state.sun,Fe.sunLightShadows.value=j.state.sunShadow,Fe.directionalLights.value=j.state.directional,Fe.directionalLightShadows.value=j.state.directionalShadow,Fe.spotLights.value=j.state.spot,Fe.spotLightShadows.value=j.state.spotShadow,Fe.rectAreaLights.value=j.state.rectArea,Fe.ltc_1.value=j.state.rectAreaLTC1,Fe.ltc_2.value=j.state.rectAreaLTC2,Fe.pointLights.value=j.state.point,Fe.pointLightShadows.value=j.state.pointShadow,Fe.hemisphereLights.value=j.state.hemi,Fe.sunShadowMatrix.value=j.state.sunShadowMatrix,Fe.sunShadowCascade.value=j.state.sunShadowCascade,Fe.directionalShadowMatrix.value=j.state.directionalShadowMatrix,Fe.spotLightMatrix.value=j.state.spotLightMatrix,Fe.spotLightMap.value=j.state.spotLightMap,Fe.pointShadowMatrix.value=j.state.pointShadowMatrix),Q.lightProbeGrid=E.state.lightProbeGridArray.length>0,Q.currentProgram=nt,Q.uniformsList=null,nt}function dt(A){if(A.uniformsList===null){let V=A.currentProgram.getUniforms();A.uniformsList=ds.seqWithValue(V.seq,A.uniforms)}return A.uniformsList}function _n(A,V){let ne=te.get(A);ne.outputColorSpace=V.outputColorSpace,ne.batching=V.batching,ne.batchingColor=V.batchingColor,ne.instancing=V.instancing,ne.instancingColor=V.instancingColor,ne.instancingMorph=V.instancingMorph,ne.skinning=V.skinning,ne.morphTargets=V.morphTargets,ne.morphNormals=V.morphNormals,ne.morphColors=V.morphColors,ne.morphTargetsCount=V.morphTargetsCount,ne.numClippingPlanes=V.numClippingPlanes,ne.numIntersection=V.numClipIntersection,ne.vertexAlphas=V.vertexAlphas,ne.vertexTangents=V.vertexTangents,ne.toneMapping=V.toneMapping}function vi(A,V){if(A.length===0)return null;if(A.length===1)return A[0].texture!==null?A[0]:null;w.setFromMatrixPosition(V.matrixWorld);for(let ne=0,Q=A.length;ne<Q;ne++){let j=A[ne];if(j.texture!==null&&j.boundingBox.containsPoint(w))return j}return null}function ir(A,V,ne,Q,j){V.isScene!==!0&&(V=Wt),ae.resetTextureUnits();let Re=V.fog,ke=Q.isMeshStandardMaterial||Q.isMeshLambertMaterial||Q.isMeshPhongMaterial?V.environment:null,Ee=ee===null?y.outputColorSpace:ee.isXRRenderTarget===!0?ee.texture.colorSpace:ut.workingColorSpace,Ae=Q.isMeshStandardMaterial||Q.isMeshLambertMaterial&&!Q.envMap||Q.isMeshPhongMaterial&&!Q.envMap,Ue=ve.get(Q.envMap||ke,Ae),et=Q.vertexColors===!0&&!!ne.attributes.color&&ne.attributes.color.itemSize===4,nt=!!ne.attributes.tangent&&(!!Q.normalMap||Q.anisotropy>0),Fe=!!ne.morphAttributes.position,xt=!!ne.morphAttributes.normal,nn=!!ne.morphAttributes.color,Dt=ni;Q.toneMapped&&(ee===null||ee.isXRRenderTarget===!0)&&(Dt=y.toneMapping);let It=ne.morphAttributes.position||ne.morphAttributes.normal||ne.morphAttributes.color,yn=It!==void 0?It.length:0,De=te.get(Q),Rn=E.state.lights;if(st===!0&&(gt===!0||A!==ie)){let Lt=A===ie&&Q.id===J;re.setState(Q,A,Lt)}let vt=!1;Q.version===De.__version?(De.needsLights&&De.lightsStateVersion!==Rn.state.version||De.outputColorSpace!==Ee||j.isBatchedMesh&&De.batching===!1||!j.isBatchedMesh&&De.batching===!0||j.isBatchedMesh&&De.batchingColor===!0&&j._colorsTexture===null||j.isBatchedMesh&&De.batchingColor===!1&&j._colorsTexture!==null||j.isInstancedMesh&&De.instancing===!1||!j.isInstancedMesh&&De.instancing===!0||j.isSkinnedMesh&&De.skinning===!1||!j.isSkinnedMesh&&De.skinning===!0||j.isInstancedMesh&&De.instancingColor===!0&&j.instanceColor===null||j.isInstancedMesh&&De.instancingColor===!1&&j.instanceColor!==null||j.isInstancedMesh&&De.instancingMorph===!0&&j.morphTexture===null||j.isInstancedMesh&&De.instancingMorph===!1&&j.morphTexture!==null||De.envMap!==Ue||Q.fog===!0&&De.fog!==Re||De.numClippingPlanes!==void 0&&(De.numClippingPlanes!==re.numPlanes||De.numIntersection!==re.numIntersection)||De.vertexAlphas!==et||De.vertexTangents!==nt||De.morphTargets!==Fe||De.morphNormals!==xt||De.morphColors!==nn||De.toneMapping!==Dt||De.morphTargetsCount!==yn||!!De.lightProbeGrid!=E.state.lightProbeGridArray.length>0)&&(vt=!0):(vt=!0,De.__version=Q.version);let Wn=De.currentProgram;vt===!0&&(Wn=Ht(Q,V,j),I&&Q.isNodeMaterial&&I.onUpdateProgram(Q,Wn,De));let oi=!1,Ii=!1,Ar=!1,Ct=Wn.getUniforms(),Kt=De.uniforms;if(b.useProgram(Wn.program)&&(oi=!0,Ii=!0,Ar=!0),Q.id!==J&&(J=Q.id,Ii=!0),De.needsLights){let Lt=vi(E.state.lightProbeGridArray,j);De.lightProbeGrid!==Lt&&(De.lightProbeGrid=Lt,Ii=!0)}if(oi||ie!==A){b.buffers.depth.getReversed()&&A.reversedDepth!==!0&&(A._reversedDepth=!0,A.updateProjectionMatrix()),Ct.setValue(W,"projectionMatrix",A.projectionMatrix),Ct.setValue(W,"viewMatrix",A.matrixWorldInverse);let ki=Ct.map.cameraPosition;ki!==void 0&&ki.setValue(W,Tt.setFromMatrixPosition(A.matrixWorld)),F.logarithmicDepthBuffer&&Ct.setValue(W,"logDepthBufFC",2/(Math.log(A.far+1)/Math.LN2)),(Q.isMeshPhongMaterial||Q.isMeshToonMaterial||Q.isMeshLambertMaterial||Q.isMeshBasicMaterial||Q.isMeshStandardMaterial||Q.isShaderMaterial)&&Ct.setValue(W,"isOrthographic",A.isOrthographicCamera===!0),ie!==A&&(ie=A,Ii=!0,Ar=!0)}if(De.needsLights&&(Rn.state.sunShadowMap.length>0&&Ct.setValue(W,"sunShadowMap",Rn.state.sunShadowMap,ae),Rn.state.directionalShadowMap.length>0&&Ct.setValue(W,"directionalShadowMap",Rn.state.directionalShadowMap,ae),Rn.state.spotShadowMap.length>0&&Ct.setValue(W,"spotShadowMap",Rn.state.spotShadowMap,ae),Rn.state.pointShadowMap.length>0&&Ct.setValue(W,"pointShadowMap",Rn.state.pointShadowMap,ae)),j.isSkinnedMesh){Ct.setOptional(W,j,"bindMatrix"),Ct.setOptional(W,j,"bindMatrixInverse");let Lt=j.skeleton;Lt&&(Lt.boneTexture===null&&Lt.computeBoneTexture(),Ct.setValue(W,"boneTexture",Lt.boneTexture,ae))}j.isBatchedMesh&&(Ct.setOptional(W,j,"batchingTexture"),Ct.setValue(W,"batchingTexture",j._matricesTexture,ae),Ct.setOptional(W,j,"batchingIdTexture"),Ct.setValue(W,"batchingIdTexture",j._indirectTexture,ae),Ct.setOptional(W,j,"batchingColorTexture"),j._colorsTexture!==null&&Ct.setValue(W,"batchingColorTexture",j._colorsTexture,ae));let Li=ne.morphAttributes;if((Li.position!==void 0||Li.normal!==void 0||Li.color!==void 0)&&B.update(j,ne,Wn),(Ii||De.receiveShadow!==j.receiveShadow)&&(De.receiveShadow=j.receiveShadow,Ct.setValue(W,"receiveShadow",j.receiveShadow)),(Q.isMeshStandardMaterial||Q.isMeshLambertMaterial||Q.isMeshPhongMaterial)&&Q.envMap===null&&V.environment!==null&&(Kt.envMapIntensity.value=V.environmentIntensity),Kt.dfgLUT!==void 0&&(Kt.dfgLUT.value=n_()),Ii){if(Ct.setValue(W,"toneMappingExposure",y.toneMappingExposure),De.needsLights&&zl(Kt,Ar),Re&&Q.fog===!0&&z.refreshFogUniforms(Kt,Re),z.refreshMaterialUniforms(Kt,Q,le,oe,E.state.transmissionRenderTarget[A.id]),De.needsLights&&De.lightProbeGrid){let Lt=De.lightProbeGrid;Kt.probesSH.value=Lt.texture,Kt.probesMin.value.copy(Lt.boundingBox.min),Kt.probesMax.value.copy(Lt.boundingBox.max),Kt.probesResolution.value.copy(Lt.resolution)}ds.upload(W,dt(De),Kt,ae)}if(Q.isShaderMaterial&&Q.uniformsNeedUpdate===!0&&(ds.upload(W,dt(De),Kt,ae),Q.uniformsNeedUpdate=!1),Q.isSpriteMaterial&&Ct.setValue(W,"center",j.center),Ct.setValue(W,"modelViewMatrix",j.modelViewMatrix),Ct.setValue(W,"normalMatrix",j.normalMatrix),Ct.setValue(W,"modelMatrix",j.matrixWorld),Q.uniformsGroups!==void 0){let Lt=Q.uniformsGroups;for(let ki=0,Rr=Lt.length;ki<Rr;ki++){let Du=Lt[ki];he.update(Du,Wn),he.bind(Du,Wn)}}return Wn}function zl(A,V){A.ambientLightColor.needsUpdate=V,A.lightProbe.needsUpdate=V,A.sunLights.needsUpdate=V,A.sunLightShadows.needsUpdate=V,A.directionalLights.needsUpdate=V,A.directionalLightShadows.needsUpdate=V,A.pointLights.needsUpdate=V,A.pointLightShadows.needsUpdate=V,A.spotLights.needsUpdate=V,A.spotLightShadows.needsUpdate=V,A.rectAreaLights.needsUpdate=V,A.hemisphereLights.needsUpdate=V}function To(A){return A.isMeshLambertMaterial||A.isMeshToonMaterial||A.isMeshPhongMaterial||A.isMeshStandardMaterial||A.isShadowMaterial||A.isShaderMaterial&&A.lights===!0}this.getActiveCubeFace=function(){return H},this.getActiveMipmapLevel=function(){return $},this.getRenderTarget=function(){return ee},this.setRenderTargetTextures=function(A,V,ne){let Q=te.get(A);Q.__autoAllocateDepthBuffer=A.resolveDepthBuffer===!1,Q.__autoAllocateDepthBuffer===!1&&(Q.__useRenderToTexture=!1),te.get(A.texture).__webglTexture=V,te.get(A.depthTexture).__webglTexture=Q.__autoAllocateDepthBuffer?void 0:ne,Q.__hasExternalTextures=!0},this.setRenderTargetFramebuffer=function(A,V){let ne=te.get(A);ne.__webglFramebuffer=V,ne.__useDefaultFramebuffer=V===void 0},this.setRenderTarget=function(A,V=0,ne=0){ee=A,H=V,$=ne;let Q=null,j=!1,Re=!1;if(A){let Ee=te.get(A);if(Ee.__useDefaultFramebuffer!==void 0){b.bindFramebuffer(W.FRAMEBUFFER,Ee.__webglFramebuffer),ce.copy(A.viewport),we.copy(A.scissor),pe=A.scissorTest,b.viewport(ce),b.scissor(we),b.setScissorTest(pe),J=-1;return}else if(Ee.__webglFramebuffer===void 0)ae.setupRenderTarget(A);else if(Ee.__hasExternalTextures)ae.rebindTextures(A,te.get(A.texture).__webglTexture,te.get(A.depthTexture).__webglTexture);else if(A.depthBuffer){let et=A.depthTexture;if(Ee.__boundDepthTexture!==et){if(et!==null&&te.has(et)&&(A.width!==et.image.width||A.height!==et.image.height))throw new Error("THREE.WebGLRenderer: Attached DepthTexture is initialized to the incorrect size.");ae.setupDepthRenderbuffer(A)}}let Ae=A.texture;(Ae.isData3DTexture||Ae.isDataArrayTexture||Ae.isCompressedArrayTexture)&&(Re=!0);let Ue=te.get(A).__webglFramebuffer;A.isWebGLCubeRenderTarget?(Array.isArray(Ue[V])?Q=Ue[V][ne]:Q=Ue[V],j=!0):A.samples>0&&ae.useMultisampledRTT(A)===!1?Q=te.get(A).__webglMultisampledFramebuffer:Array.isArray(Ue)?Q=Ue[ne]:Q=Ue,ce.copy(A.viewport),we.copy(A.scissor),pe=A.scissorTest}else ce.copy(Me).multiplyScalar(le).floor(),we.copy(ze).multiplyScalar(le).floor(),pe=Ut;if(ne!==0&&(Q=k),b.bindFramebuffer(W.FRAMEBUFFER,Q)&&b.drawBuffers(A,Q),b.viewport(ce),b.scissor(we),b.setScissorTest(pe),j){let Ee=te.get(A.texture);W.framebufferTexture2D(W.FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_CUBE_MAP_POSITIVE_X+V,Ee.__webglTexture,ne)}else if(Re){let Ee=V;for(let Ae=0;Ae<A.textures.length;Ae++){let Ue=te.get(A.textures[Ae]);W.framebufferTextureLayer(W.FRAMEBUFFER,W.COLOR_ATTACHMENT0+Ae,Ue.__webglTexture,ne,Ee)}}else if(A!==null&&ne!==0){let Ee=te.get(A.texture);W.framebufferTexture2D(W.FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_2D,Ee.__webglTexture,ne)}J=-1};function Tr(A){let V=te.get(A);return(V.__readFormat!==A.format||V.__readType!==A.type)&&(V.__readFormat=A.format,V.__readType=A.type,V.__formatReadable=F.textureFormatReadable(A.format),V.__typeReadable=F.textureTypeReadable(A.type)),V}this.readRenderTargetPixels=function(A,V,ne,Q,j,Re,ke,Ee=0){if(!(A&&A.isWebGLRenderTarget)){Ze("WebGLRenderer.readRenderTargetPixels: renderTarget is not THREE.WebGLRenderTarget.");return}let Ae=te.get(A).__webglFramebuffer;if(A.isWebGLCubeRenderTarget&&ke!==void 0&&(Ae=Ae[ke]),Ae){b.bindFramebuffer(W.FRAMEBUFFER,Ae);try{let Ue=A.textures[Ee],et=Ue.format,nt=Ue.type;A.textures.length>1&&W.readBuffer(W.COLOR_ATTACHMENT0+Ee);let Fe=Tr(Ue);if(Fe.__formatReadable===!1){Ze("WebGLRenderer.readRenderTargetPixels: renderTarget is not in RGBA or implementation defined format.");return}if(Fe.__typeReadable===!1){Ze("WebGLRenderer.readRenderTargetPixels: renderTarget is not in UnsignedByteType or implementation defined type.");return}V>=0&&V<=A.width-Q&&ne>=0&&ne<=A.height-j&&W.readPixels(V,ne,Q,j,ge.convert(et),ge.convert(nt),Re)}finally{let Ue=ee!==null?te.get(ee).__webglFramebuffer:null;b.bindFramebuffer(W.FRAMEBUFFER,Ue)}}},this.readRenderTargetPixelsAsync=async function(A,V,ne,Q,j,Re,ke,Ee=0){if(!(A&&A.isWebGLRenderTarget))throw new Error("THREE.WebGLRenderer.readRenderTargetPixels: renderTarget is not THREE.WebGLRenderTarget.");let Ae=te.get(A).__webglFramebuffer;if(A.isWebGLCubeRenderTarget&&ke!==void 0&&(Ae=Ae[ke]),Ae)if(V>=0&&V<=A.width-Q&&ne>=0&&ne<=A.height-j){b.bindFramebuffer(W.FRAMEBUFFER,Ae);let Ue=A.textures[Ee],et=Ue.format,nt=Ue.type;A.textures.length>1&&W.readBuffer(W.COLOR_ATTACHMENT0+Ee);let Fe=Tr(Ue);if(Fe.__formatReadable===!1)throw new Error("THREE.WebGLRenderer.readRenderTargetPixelsAsync: renderTarget is not in RGBA or implementation defined format.");if(Fe.__typeReadable===!1)throw new Error("THREE.WebGLRenderer.readRenderTargetPixelsAsync: renderTarget is not in UnsignedByteType or implementation defined type.");let xt=W.createBuffer();W.bindBuffer(W.PIXEL_PACK_BUFFER,xt),W.bufferData(W.PIXEL_PACK_BUFFER,Re.byteLength,W.STREAM_READ),W.readPixels(V,ne,Q,j,ge.convert(et),ge.convert(nt),0),W.bindBuffer(W.PIXEL_PACK_BUFFER,null);let nn=ee!==null?te.get(ee).__webglFramebuffer:null;b.bindFramebuffer(W.FRAMEBUFFER,nn);let Dt=W.fenceSync(W.SYNC_GPU_COMMANDS_COMPLETE,0);return W.flush(),await dd(W,Dt,4),W.bindBuffer(W.PIXEL_PACK_BUFFER,xt),W.getBufferSubData(W.PIXEL_PACK_BUFFER,0,Re),W.bindBuffer(W.PIXEL_PACK_BUFFER,null),W.deleteBuffer(xt),W.deleteSync(Dt),Re}else throw new Error("THREE.WebGLRenderer.readRenderTargetPixelsAsync: requested read bounds are out of range.")},this.copyFramebufferToTexture=function(A,V=null,ne=0){let Q=Math.pow(2,-ne),j=Math.floor(A.image.width*Q),Re=Math.floor(A.image.height*Q),ke=V!==null?V.x:0,Ee=V!==null?V.y:0;ae.setTexture2D(A,0),W.copyTexSubImage2D(W.TEXTURE_2D,ne,0,0,ke,Ee,j,Re),b.unbindTexture()},this.copyTextureToTexture=function(A,V,ne=null,Q=null,j=0,Re=0){let ke,Ee,Ae,Ue,et,nt,Fe,xt,nn,Dt=A.isCompressedTexture?A.mipmaps[Re]:A.image;if(ne!==null)ke=ne.max.x-ne.min.x,Ee=ne.max.y-ne.min.y,Ae=ne.isBox3?ne.max.z-ne.min.z:1,Ue=ne.min.x,et=ne.min.y,nt=ne.isBox3?ne.min.z:0;else{let Kt=Math.pow(2,-j);ke=Math.floor(Dt.width*Kt),Ee=Math.floor(Dt.height*Kt),A.isDataArrayTexture?Ae=Dt.depth:A.isData3DTexture?Ae=Math.floor(Dt.depth*Kt):Ae=1,Ue=0,et=0,nt=0}Q!==null?(Fe=Q.x,xt=Q.y,nn=Q.z):(Fe=0,xt=0,nn=0);let It=ge.convert(V.format),yn=ge.convert(V.type),De;V.isData3DTexture?(ae.setTexture3D(V,0),De=W.TEXTURE_3D):V.isDataArrayTexture||V.isCompressedArrayTexture?(ae.setTexture2DArray(V,0),De=W.TEXTURE_2D_ARRAY):(ae.setTexture2D(V,0),De=W.TEXTURE_2D),b.activeTexture(W.TEXTURE0),b.pixelStorei(W.UNPACK_FLIP_Y_WEBGL,V.flipY),b.pixelStorei(W.UNPACK_PREMULTIPLY_ALPHA_WEBGL,V.premultiplyAlpha),b.pixelStorei(W.UNPACK_ALIGNMENT,V.unpackAlignment);let Rn=b.getParameter(W.UNPACK_ROW_LENGTH),vt=b.getParameter(W.UNPACK_IMAGE_HEIGHT),Wn=b.getParameter(W.UNPACK_SKIP_PIXELS),oi=b.getParameter(W.UNPACK_SKIP_ROWS),Ii=b.getParameter(W.UNPACK_SKIP_IMAGES);b.pixelStorei(W.UNPACK_ROW_LENGTH,Dt.width),b.pixelStorei(W.UNPACK_IMAGE_HEIGHT,Dt.height),b.pixelStorei(W.UNPACK_SKIP_PIXELS,Ue),b.pixelStorei(W.UNPACK_SKIP_ROWS,et),b.pixelStorei(W.UNPACK_SKIP_IMAGES,nt);let Ar=A.isDataArrayTexture||A.isData3DTexture,Ct=V.isDataArrayTexture||V.isData3DTexture;if(A.isDepthTexture){let Kt=te.get(A),Li=te.get(V),Lt=te.get(Kt.__renderTarget),ki=te.get(Li.__renderTarget);b.bindFramebuffer(W.READ_FRAMEBUFFER,Lt.__webglFramebuffer),b.bindFramebuffer(W.DRAW_FRAMEBUFFER,ki.__webglFramebuffer);for(let Rr=0;Rr<Ae;Rr++)Ar&&(W.framebufferTextureLayer(W.READ_FRAMEBUFFER,W.COLOR_ATTACHMENT0,te.get(A).__webglTexture,j,nt+Rr),W.framebufferTextureLayer(W.DRAW_FRAMEBUFFER,W.COLOR_ATTACHMENT0,te.get(V).__webglTexture,Re,nn+Rr)),W.blitFramebuffer(Ue,et,ke,Ee,Fe,xt,ke,Ee,W.DEPTH_BUFFER_BIT,W.NEAREST);b.bindFramebuffer(W.READ_FRAMEBUFFER,null),b.bindFramebuffer(W.DRAW_FRAMEBUFFER,null)}else if(j!==0||A.isRenderTargetTexture||te.has(A)){let Kt=te.get(A),Li=te.get(V);b.bindFramebuffer(W.READ_FRAMEBUFFER,D),b.bindFramebuffer(W.DRAW_FRAMEBUFFER,N);for(let Lt=0;Lt<Ae;Lt++)Ar?W.framebufferTextureLayer(W.READ_FRAMEBUFFER,W.COLOR_ATTACHMENT0,Kt.__webglTexture,j,nt+Lt):W.framebufferTexture2D(W.READ_FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_2D,Kt.__webglTexture,j),Ct?W.framebufferTextureLayer(W.DRAW_FRAMEBUFFER,W.COLOR_ATTACHMENT0,Li.__webglTexture,Re,nn+Lt):W.framebufferTexture2D(W.DRAW_FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_2D,Li.__webglTexture,Re),j!==0?W.blitFramebuffer(Ue,et,ke,Ee,Fe,xt,ke,Ee,W.COLOR_BUFFER_BIT,W.NEAREST):Ct?W.copyTexSubImage3D(De,Re,Fe,xt,nn+Lt,Ue,et,ke,Ee):W.copyTexSubImage2D(De,Re,Fe,xt,Ue,et,ke,Ee);b.bindFramebuffer(W.READ_FRAMEBUFFER,null),b.bindFramebuffer(W.DRAW_FRAMEBUFFER,null)}else Ct?A.isDataTexture||A.isData3DTexture?W.texSubImage3D(De,Re,Fe,xt,nn,ke,Ee,Ae,It,yn,Dt.data):V.isCompressedArrayTexture?W.compressedTexSubImage3D(De,Re,Fe,xt,nn,ke,Ee,Ae,It,Dt.data):W.texSubImage3D(De,Re,Fe,xt,nn,ke,Ee,Ae,It,yn,Dt):A.isDataTexture?W.texSubImage2D(W.TEXTURE_2D,Re,Fe,xt,ke,Ee,It,yn,Dt.data):A.isCompressedTexture?W.compressedTexSubImage2D(W.TEXTURE_2D,Re,Fe,xt,Dt.width,Dt.height,It,Dt.data):W.texSubImage2D(W.TEXTURE_2D,Re,Fe,xt,ke,Ee,It,yn,Dt);b.pixelStorei(W.UNPACK_ROW_LENGTH,Rn),b.pixelStorei(W.UNPACK_IMAGE_HEIGHT,vt),b.pixelStorei(W.UNPACK_SKIP_PIXELS,Wn),b.pixelStorei(W.UNPACK_SKIP_ROWS,oi),b.pixelStorei(W.UNPACK_SKIP_IMAGES,Ii),Re===0&&V.generateMipmaps&&W.generateMipmap(De),b.unbindTexture()},this.initRenderTarget=function(A){te.get(A).__webglFramebuffer===void 0&&ae.setupRenderTarget(A)},this.initTexture=function(A){A.isCubeTexture?ae.setTextureCube(A,0):A.isData3DTexture?ae.setTexture3D(A,0):A.isDataArrayTexture||A.isCompressedArrayTexture?ae.setTexture2DArray(A,0):ae.setTexture2D(A,0),b.unbindTexture()},this.resetState=function(){H=0,$=0,ee=null,b.reset(),_e.reset()},typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("observe",{detail:this}))}get coordinateSystem(){return ei}get outputColorSpace(){return this._outputColorSpace}set outputColorSpace(e){this._outputColorSpace=e;let t=this.getContext();t.drawingBufferColorSpace=ut._getDrawingBufferColorSpace(e),t.unpackColorSpace=ut._getUnpackColorSpace()}};var Xd={type:"change"},xu={type:"start"},$d={type:"end"},El=new Hi,qd=new Hn,i_=Math.cos(70*Kc.DEG2RAD),hn=new G,On=2*Math.PI,At={NONE:-1,ROTATE:0,DOLLY:1,PAN:2,TOUCH_ROTATE:3,TOUCH_PAN:4,TOUCH_DOLLY_PAN:5,TOUCH_DOLLY_ROTATE:6},gu=1e-6,Tl=class extends to{constructor(e,t=null){super(e,t),this.state=At.NONE,this.target=new G,this.cursor=new G,this.minDistance=0,this.maxDistance=1/0,this.minZoom=0,this.maxZoom=1/0,this.minTargetRadius=0,this.maxTargetRadius=1/0,this.minPolarAngle=0,this.maxPolarAngle=Math.PI,this.minAzimuthAngle=-1/0,this.maxAzimuthAngle=1/0,this.enableDamping=!1,this.dampingFactor=.05,this.enableZoom=!0,this.zoomSpeed=1,this.enableRotate=!0,this.rotateSpeed=1,this.keyRotateSpeed=1,this.enablePan=!0,this.panSpeed=1,this.screenSpacePanning=!0,this.keyPanSpeed=7,this.zoomToCursor=!1,this.autoRotate=!1,this.autoRotateSpeed=2,this.keys={LEFT:"ArrowLeft",UP:"ArrowUp",RIGHT:"ArrowRight",BOTTOM:"ArrowDown"},this.mouseButtons={LEFT:Yi.ROTATE,MIDDLE:Yi.DOLLY,RIGHT:Yi.PAN},this.touches={ONE:Zi.ROTATE,TWO:Zi.DOLLY_PAN},this.target0=this.target.clone(),this.position0=this.object.position.clone(),this.zoom0=this.object.zoom,this._cursorStyle="auto",this._domElementKeyEvents=null,this._lastPosition=new G,this._lastQuaternion=new an,this._lastTargetPosition=new G,this._quat=new an().setFromUnitVectors(e.up,new G(0,1,0)),this._quatInverse=this._quat.clone().invert(),this._spherical=new is,this._sphericalDelta=new is,this._scale=1,this._panOffset=new G,this._rotateStart=new We,this._rotateEnd=new We,this._rotateDelta=new We,this._panStart=new We,this._panEnd=new We,this._panDelta=new We,this._dollyStart=new We,this._dollyEnd=new We,this._dollyDelta=new We,this._dollyDirection=new G,this._mouse=new We,this._performCursorZoom=!1,this._pointers=[],this._pointerPositions={},this._controlActive=!1,this._onPointerMove=s_.bind(this),this._onPointerDown=r_.bind(this),this._onPointerUp=o_.bind(this),this._onContextMenu=f_.bind(this),this._onMouseWheel=c_.bind(this),this._onKeyDown=u_.bind(this),this._onTouchStart=h_.bind(this),this._onTouchMove=d_.bind(this),this._onMouseDown=a_.bind(this),this._onMouseMove=l_.bind(this),this._interceptControlDown=p_.bind(this),this._interceptControlUp=m_.bind(this),this.domElement!==null&&this.connect(this.domElement),this.update()}set cursorStyle(e){this._cursorStyle=e,e==="grab"?this.domElement.style.cursor="grab":this.domElement.style.cursor="auto"}get cursorStyle(){return this._cursorStyle}connect(e){super.connect(e),this.domElement.addEventListener("pointerdown",this._onPointerDown),this.domElement.addEventListener("pointercancel",this._onPointerUp),this.domElement.addEventListener("contextmenu",this._onContextMenu),this.domElement.addEventListener("wheel",this._onMouseWheel,{passive:!1}),this.domElement.getRootNode().addEventListener("keydown",this._interceptControlDown,{passive:!0,capture:!0}),this.domElement.style.touchAction="none"}disconnect(){this.state=At.NONE,this.domElement.removeEventListener("pointerdown",this._onPointerDown),this.domElement.ownerDocument.removeEventListener("pointermove",this._onPointerMove),this.domElement.ownerDocument.removeEventListener("pointerup",this._onPointerUp),this.domElement.removeEventListener("pointercancel",this._onPointerUp),this.domElement.removeEventListener("wheel",this._onMouseWheel),this.domElement.removeEventListener("contextmenu",this._onContextMenu),this.stopListenToKeyEvents();let e=this.domElement.getRootNode();e.removeEventListener("keydown",this._interceptControlDown,{capture:!0}),e.removeEventListener("keyup",this._interceptControlUp,{capture:!0}),this._controlActive=!1,this._pointers.length=0,this._pointerPositions={},this.domElement.style.touchAction="",this.domElement.style.cursor="auto"}dispose(){this.disconnect()}getPolarAngle(){return this._spherical.phi}getAzimuthalAngle(){return this._spherical.theta}getDistance(){return this.object.position.distanceTo(this.target)}listenToKeyEvents(e){e.addEventListener("keydown",this._onKeyDown),this._domElementKeyEvents=e}stopListenToKeyEvents(){this._domElementKeyEvents!==null&&(this._domElementKeyEvents.removeEventListener("keydown",this._onKeyDown),this._domElementKeyEvents=null)}saveState(){this.target0.copy(this.target),this.position0.copy(this.object.position),this.zoom0=this.object.zoom}reset(){this.target.copy(this.target0),this.object.position.copy(this.position0),this.object.zoom=this.zoom0,this.object.updateProjectionMatrix(),this.dispatchEvent(Xd),this.update(),this.state=At.NONE}pan(e,t){this._pan(e,t),this.update()}dollyIn(e){this._dollyIn(e),this.update()}dollyOut(e){this._dollyOut(e),this.update()}rotateLeft(e){this._rotateLeft(e),this.update()}rotateUp(e){this._rotateUp(e),this.update()}update(e=null){let t=this.object.position;hn.copy(t).sub(this.target),hn.applyQuaternion(this._quat),this._spherical.setFromVector3(hn),this.autoRotate&&this.state===At.NONE&&this._rotateLeft(this._getAutoRotationAngle(e)),this.enableDamping?(this._spherical.theta+=this._sphericalDelta.theta*this.dampingFactor,this._spherical.phi+=this._sphericalDelta.phi*this.dampingFactor):(this._spherical.theta+=this._sphericalDelta.theta,this._spherical.phi+=this._sphericalDelta.phi);let i=this.minAzimuthAngle,r=this.maxAzimuthAngle;isFinite(i)&&isFinite(r)&&(i<-Math.PI?i+=On:i>Math.PI&&(i-=On),r<-Math.PI?r+=On:r>Math.PI&&(r-=On),i<=r?this._spherical.theta=Math.max(i,Math.min(r,this._spherical.theta)):this._spherical.theta=this._spherical.theta>(i+r)/2?Math.max(i,this._spherical.theta):Math.min(r,this._spherical.theta)),this._spherical.phi=Math.max(this.minPolarAngle,Math.min(this.maxPolarAngle,this._spherical.phi)),this._spherical.makeSafe(),this.enableDamping===!0?this.target.addScaledVector(this._panOffset,this.dampingFactor):this.target.add(this._panOffset),this.target.sub(this.cursor),this.target.clampLength(this.minTargetRadius,this.maxTargetRadius),this.target.add(this.cursor);let s=!1;if(this.zoomToCursor&&this._performCursorZoom||this.object.isOrthographicCamera)this._spherical.radius=this._clampDistance(this._spherical.radius);else{let o=this._spherical.radius;this._spherical.radius=this._clampDistance(this._spherical.radius*this._scale),s=o!=this._spherical.radius}if(hn.setFromSpherical(this._spherical),hn.applyQuaternion(this._quatInverse),t.copy(this.target).add(hn),this.object.lookAt(this.target),this.enableDamping===!0?(this._sphericalDelta.theta*=1-this.dampingFactor,this._sphericalDelta.phi*=1-this.dampingFactor,this._panOffset.multiplyScalar(1-this.dampingFactor)):(this._sphericalDelta.set(0,0,0),this._panOffset.set(0,0,0)),this.zoomToCursor&&this._performCursorZoom){let o=null;if(this.object.isPerspectiveCamera){let a=hn.length();o=this._clampDistance(a*this._scale);let l=a-o;this.object.position.addScaledVector(this._dollyDirection,l),this.object.updateMatrixWorld(),s=!!l}else if(this.object.isOrthographicCamera){let a=new G(this._mouse.x,this._mouse.y,0);a.unproject(this.object);let l=this.object.zoom;this.object.zoom=Math.max(this.minZoom,Math.min(this.maxZoom,this.object.zoom/this._scale)),this.object.updateProjectionMatrix(),s=l!==this.object.zoom;let c=new G(this._mouse.x,this._mouse.y,0);c.unproject(this.object),this.object.position.sub(c).add(a),this.object.updateMatrixWorld(),o=hn.length()}else console.warn("WARNING: OrbitControls.js encountered an unknown camera type - zoom to cursor disabled."),this.zoomToCursor=!1;o!==null&&(this.screenSpacePanning?this.target.set(0,0,-1).transformDirection(this.object.matrix).multiplyScalar(o).add(this.object.position):(El.origin.copy(this.object.position),El.direction.set(0,0,-1).transformDirection(this.object.matrix),Math.abs(this.object.up.dot(El.direction))<i_?this.object.lookAt(this.target):(qd.setFromNormalAndCoplanarPoint(this.object.up,this.target),El.intersectPlane(qd,this.target))))}else if(this.object.isOrthographicCamera){let o=this.object.zoom;this.object.zoom=Math.max(this.minZoom,Math.min(this.maxZoom,this.object.zoom/this._scale)),o!==this.object.zoom&&(this.object.updateProjectionMatrix(),s=!0)}return this._scale=1,this._performCursorZoom=!1,s||this._lastPosition.distanceToSquared(this.object.position)>gu||8*(1-this._lastQuaternion.dot(this.object.quaternion))>gu||this._lastTargetPosition.distanceToSquared(this.target)>gu?(this.dispatchEvent(Xd),this._lastPosition.copy(this.object.position),this._lastQuaternion.copy(this.object.quaternion),this._lastTargetPosition.copy(this.target),!0):!1}_getAutoRotationAngle(e){return e!==null?On/60*this.autoRotateSpeed*e:On/60/60*this.autoRotateSpeed}_getZoomScale(e){let t=Math.abs(e*.01);return Math.pow(.95,this.zoomSpeed*t)}_rotateLeft(e){this._sphericalDelta.theta-=e}_rotateUp(e){this._sphericalDelta.phi-=e}_panLeft(e,t){hn.setFromMatrixColumn(t,0),hn.multiplyScalar(-e),this._panOffset.add(hn)}_panUp(e,t){this.screenSpacePanning===!0?hn.setFromMatrixColumn(t,1):(hn.setFromMatrixColumn(t,0),hn.crossVectors(this.object.up,hn)),hn.multiplyScalar(e),this._panOffset.add(hn)}_pan(e,t){let i=this.domElement;if(this.object.isPerspectiveCamera){let r=this.object.position;hn.copy(r).sub(this.target);let s=hn.length();s*=Math.tan(this.object.fov/2*Math.PI/180),this._panLeft(2*e*s/i.clientHeight,this.object.matrix),this._panUp(2*t*s/i.clientHeight,this.object.matrix)}else this.object.isOrthographicCamera?(this._panLeft(e*(this.object.right-this.object.left)/this.object.zoom/i.clientWidth,this.object.matrix),this._panUp(t*(this.object.top-this.object.bottom)/this.object.zoom/i.clientHeight,this.object.matrix)):(console.warn("WARNING: OrbitControls.js encountered an unknown camera type - pan disabled."),this.enablePan=!1)}_dollyOut(e){this.object.isPerspectiveCamera||this.object.isOrthographicCamera?this._scale/=e:(console.warn("WARNING: OrbitControls.js encountered an unknown camera type - dolly/zoom disabled."),this.enableZoom=!1)}_dollyIn(e){this.object.isPerspectiveCamera||this.object.isOrthographicCamera?this._scale*=e:(console.warn("WARNING: OrbitControls.js encountered an unknown camera type - dolly/zoom disabled."),this.enableZoom=!1)}_updateZoomParameters(e,t){if(!this.zoomToCursor)return;this._performCursorZoom=!0;let i=this.domElement.getBoundingClientRect(),r=e-i.left,s=t-i.top,o=i.width,a=i.height;this._mouse.x=r/o*2-1,this._mouse.y=-(s/a)*2+1,this._dollyDirection.set(this._mouse.x,this._mouse.y,1).unproject(this.object).sub(this.object.position).normalize()}_clampDistance(e){return Math.max(this.minDistance,Math.min(this.maxDistance,e))}_handleMouseDownRotate(e){this._rotateStart.set(e.clientX,e.clientY)}_handleMouseDownDolly(e){this._updateZoomParameters(e.clientX,e.clientX),this._dollyStart.set(e.clientX,e.clientY)}_handleMouseDownPan(e){this._panStart.set(e.clientX,e.clientY)}_handleMouseMoveRotate(e){this._rotateEnd.set(e.clientX,e.clientY),this._rotateDelta.subVectors(this._rotateEnd,this._rotateStart).multiplyScalar(this.rotateSpeed);let t=this.domElement;this._rotateLeft(On*this._rotateDelta.x/t.clientHeight),this._rotateUp(On*this._rotateDelta.y/t.clientHeight),this._rotateStart.copy(this._rotateEnd),this.update()}_handleMouseMoveDolly(e){this._dollyEnd.set(e.clientX,e.clientY),this._dollyDelta.subVectors(this._dollyEnd,this._dollyStart),this._dollyDelta.y>0?this._dollyOut(this._getZoomScale(this._dollyDelta.y)):this._dollyDelta.y<0&&this._dollyIn(this._getZoomScale(this._dollyDelta.y)),this._dollyStart.copy(this._dollyEnd),this.update()}_handleMouseMovePan(e){this._panEnd.set(e.clientX,e.clientY),this._panDelta.subVectors(this._panEnd,this._panStart).multiplyScalar(this.panSpeed),this._pan(this._panDelta.x,this._panDelta.y),this._panStart.copy(this._panEnd),this.update()}_handleMouseWheel(e){this._updateZoomParameters(e.clientX,e.clientY),e.deltaY<0?this._dollyIn(this._getZoomScale(e.deltaY)):e.deltaY>0&&this._dollyOut(this._getZoomScale(e.deltaY)),this.update()}_handleKeyDown(e){let t=!1;switch(e.code){case this.keys.UP:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateUp(On*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(0,this.keyPanSpeed),t=!0;break;case this.keys.BOTTOM:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateUp(-On*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(0,-this.keyPanSpeed),t=!0;break;case this.keys.LEFT:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateLeft(On*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(this.keyPanSpeed,0),t=!0;break;case this.keys.RIGHT:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateLeft(-On*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(-this.keyPanSpeed,0),t=!0;break}t&&(e.preventDefault(),this.update())}_handleTouchStartRotate(e){if(this._pointers.length===1)this._rotateStart.set(e.pageX,e.pageY);else{let t=this._getSecondPointerPosition(e),i=.5*(e.pageX+t.x),r=.5*(e.pageY+t.y);this._rotateStart.set(i,r)}}_handleTouchStartPan(e){if(this._pointers.length===1)this._panStart.set(e.pageX,e.pageY);else{let t=this._getSecondPointerPosition(e),i=.5*(e.pageX+t.x),r=.5*(e.pageY+t.y);this._panStart.set(i,r)}}_handleTouchStartDolly(e){let t=this._getSecondPointerPosition(e),i=e.pageX-t.x,r=e.pageY-t.y,s=Math.sqrt(i*i+r*r);this._dollyStart.set(0,s)}_handleTouchStartDollyPan(e){this.enableZoom&&this._handleTouchStartDolly(e),this.enablePan&&this._handleTouchStartPan(e)}_handleTouchStartDollyRotate(e){this.enableZoom&&this._handleTouchStartDolly(e),this.enableRotate&&this._handleTouchStartRotate(e)}_handleTouchMoveRotate(e){if(this._pointers.length==1)this._rotateEnd.set(e.pageX,e.pageY);else{let i=this._getSecondPointerPosition(e),r=.5*(e.pageX+i.x),s=.5*(e.pageY+i.y);this._rotateEnd.set(r,s)}this._rotateDelta.subVectors(this._rotateEnd,this._rotateStart).multiplyScalar(this.rotateSpeed);let t=this.domElement;this._rotateLeft(On*this._rotateDelta.x/t.clientHeight),this._rotateUp(On*this._rotateDelta.y/t.clientHeight),this._rotateStart.copy(this._rotateEnd)}_handleTouchMovePan(e){if(this._pointers.length===1)this._panEnd.set(e.pageX,e.pageY);else{let t=this._getSecondPointerPosition(e),i=.5*(e.pageX+t.x),r=.5*(e.pageY+t.y);this._panEnd.set(i,r)}this._panDelta.subVectors(this._panEnd,this._panStart).multiplyScalar(this.panSpeed),this._pan(this._panDelta.x,this._panDelta.y),this._panStart.copy(this._panEnd)}_handleTouchMoveDolly(e){let t=this._getSecondPointerPosition(e),i=e.pageX-t.x,r=e.pageY-t.y,s=Math.sqrt(i*i+r*r);this._dollyEnd.set(0,s),this._dollyDelta.set(0,Math.pow(this._dollyEnd.y/this._dollyStart.y,this.zoomSpeed)),this._dollyOut(this._dollyDelta.y),this._dollyStart.copy(this._dollyEnd);let o=(e.pageX+t.x)*.5,a=(e.pageY+t.y)*.5;this._updateZoomParameters(o,a)}_handleTouchMoveDollyPan(e){this.enableZoom&&this._handleTouchMoveDolly(e),this.enablePan&&this._handleTouchMovePan(e)}_handleTouchMoveDollyRotate(e){this.enableZoom&&this._handleTouchMoveDolly(e),this.enableRotate&&this._handleTouchMoveRotate(e)}_addPointer(e){this._pointers.push(e.pointerId)}_removePointer(e){delete this._pointerPositions[e.pointerId];for(let t=0;t<this._pointers.length;t++)if(this._pointers[t]==e.pointerId){this._pointers.splice(t,1);return}}_isTrackingPointer(e){for(let t=0;t<this._pointers.length;t++)if(this._pointers[t]==e.pointerId)return!0;return!1}_trackPointer(e){let t=this._pointerPositions[e.pointerId];t===void 0&&(t=new We,this._pointerPositions[e.pointerId]=t),t.set(e.pageX,e.pageY)}_getSecondPointerPosition(e){let t=e.pointerId===this._pointers[0]?this._pointers[1]:this._pointers[0];return this._pointerPositions[t]}_customWheelEvent(e){let t=e.deltaMode,i={clientX:e.clientX,clientY:e.clientY,deltaY:e.deltaY};switch(t){case 1:i.deltaY*=16;break;case 2:i.deltaY*=100;break}return e.ctrlKey&&!this._controlActive&&(i.deltaY*=10),i}};function r_(n){this.enabled!==!1&&(this._pointers.length===0&&(this.domElement.setPointerCapture(n.pointerId),this.domElement.ownerDocument.addEventListener("pointermove",this._onPointerMove),this.domElement.ownerDocument.addEventListener("pointerup",this._onPointerUp)),!this._isTrackingPointer(n)&&(this._addPointer(n),n.pointerType==="touch"?this._onTouchStart(n):this._onMouseDown(n),this._cursorStyle==="grab"&&(this.domElement.style.cursor="grabbing")))}function s_(n){this.enabled!==!1&&(n.pointerType==="touch"?this._onTouchMove(n):this._onMouseMove(n))}function o_(n){switch(this._removePointer(n),this._pointers.length){case 0:this.domElement.releasePointerCapture(n.pointerId),this.domElement.ownerDocument.removeEventListener("pointermove",this._onPointerMove),this.domElement.ownerDocument.removeEventListener("pointerup",this._onPointerUp),this.dispatchEvent($d),this.state=At.NONE,this._cursorStyle==="grab"&&(this.domElement.style.cursor="grab");break;case 1:let e=this._pointers[0],t=this._pointerPositions[e];this._onTouchStart({pointerId:e,pageX:t.x,pageY:t.y});break}}function a_(n){let e;switch(n.button){case 0:e=this.mouseButtons.LEFT;break;case 1:e=this.mouseButtons.MIDDLE;break;case 2:e=this.mouseButtons.RIGHT;break;default:e=-1}switch(e){case Yi.DOLLY:if(this.enableZoom===!1)return;this._handleMouseDownDolly(n),this.state=At.DOLLY;break;case Yi.ROTATE:if(n.ctrlKey||n.metaKey||n.shiftKey){if(this.enablePan===!1)return;this._handleMouseDownPan(n),this.state=At.PAN}else{if(this.enableRotate===!1)return;this._handleMouseDownRotate(n),this.state=At.ROTATE}break;case Yi.PAN:if(n.ctrlKey||n.metaKey||n.shiftKey){if(this.enableRotate===!1)return;this._handleMouseDownRotate(n),this.state=At.ROTATE}else{if(this.enablePan===!1)return;this._handleMouseDownPan(n),this.state=At.PAN}break;default:this.state=At.NONE}this.state!==At.NONE&&this.dispatchEvent(xu)}function l_(n){switch(this.state){case At.ROTATE:if(this.enableRotate===!1)return;this._handleMouseMoveRotate(n);break;case At.DOLLY:if(this.enableZoom===!1)return;this._handleMouseMoveDolly(n);break;case At.PAN:if(this.enablePan===!1)return;this._handleMouseMovePan(n);break}}function c_(n){this.enabled===!1||this.enableZoom===!1||this.state!==At.NONE||(n.preventDefault(),this.dispatchEvent(xu),this._handleMouseWheel(this._customWheelEvent(n)),this.dispatchEvent($d))}function u_(n){this.enabled!==!1&&this._handleKeyDown(n)}function h_(n){switch(this._trackPointer(n),this._pointers.length){case 1:switch(this.touches.ONE){case Zi.ROTATE:if(this.enableRotate===!1)return;this._handleTouchStartRotate(n),this.state=At.TOUCH_ROTATE;break;case Zi.PAN:if(this.enablePan===!1)return;this._handleTouchStartPan(n),this.state=At.TOUCH_PAN;break;default:this.state=At.NONE}break;case 2:switch(this.touches.TWO){case Zi.DOLLY_PAN:if(this.enableZoom===!1&&this.enablePan===!1)return;this._handleTouchStartDollyPan(n),this.state=At.TOUCH_DOLLY_PAN;break;case Zi.DOLLY_ROTATE:if(this.enableZoom===!1&&this.enableRotate===!1)return;this._handleTouchStartDollyRotate(n),this.state=At.TOUCH_DOLLY_ROTATE;break;default:this.state=At.NONE}break;default:this.state=At.NONE}this.state!==At.NONE&&this.dispatchEvent(xu)}function d_(n){switch(this._trackPointer(n),this.state){case At.TOUCH_ROTATE:if(this.enableRotate===!1)return;this._handleTouchMoveRotate(n),this.update();break;case At.TOUCH_PAN:if(this.enablePan===!1)return;this._handleTouchMovePan(n),this.update();break;case At.TOUCH_DOLLY_PAN:if(this.enableZoom===!1&&this.enablePan===!1)return;this._handleTouchMoveDollyPan(n),this.update();break;case At.TOUCH_DOLLY_ROTATE:if(this.enableZoom===!1&&this.enableRotate===!1)return;this._handleTouchMoveDollyRotate(n),this.update();break;default:this.state=At.NONE}}function f_(n){this.enabled!==!1&&n.preventDefault()}function p_(n){n.key==="Control"&&(this._controlActive=!0,this.domElement.getRootNode().addEventListener("keyup",this._interceptControlUp,{passive:!0,capture:!0}))}function m_(n){n.key==="Control"&&(this._controlActive=!1,this.domElement.getRootNode().removeEventListener("keyup",this._interceptControlUp,{passive:!0,capture:!0}))}var mo=new G;function Yn(n,e,t,i,r,s){let o=2*Math.PI*r/4,a=Math.max(s-2*r,0),l=Math.PI/4;mo.copy(e),mo[i]=0,mo.normalize();let c=.5*o/(o+a),d=1-mo.angleTo(n)/l;return Math.sign(mo[t])===1?d*c:a/(o+a)+c+c*(1-d)}var Ri=class n extends Qe{constructor(e=1,t=1,i=1,r=2,s=.1){let o=r*2+1;if(s=Math.min(e/2,t/2,i/2,s),super(1,1,1,o,o,o),this.type="RoundedBoxGeometry",this.parameters={width:e,height:t,depth:i,segments:r,radius:s},o===1)return;let a=this.toNonIndexed();this.index=null,this.attributes.position=a.attributes.position,this.attributes.normal=a.attributes.normal,this.attributes.uv=a.attributes.uv;let l=new G,c=new G,d=new G(e,t,i).divideScalar(2).subScalar(s),m=this.attributes.position.array,u=this.attributes.normal.array,g=this.attributes.uv.array,_=m.length/6,v=new G,f=.5/o;for(let h=0,M=0;h<m.length;h+=3,M+=2)switch(l.fromArray(m,h),c.copy(l),c.x-=Math.sign(c.x)*f,c.y-=Math.sign(c.y)*f,c.z-=Math.sign(c.z)*f,c.normalize(),m[h+0]=d.x*Math.sign(l.x)+c.x*s,m[h+1]=d.y*Math.sign(l.y)+c.y*s,m[h+2]=d.z*Math.sign(l.z)+c.z*s,u[h+0]=c.x,u[h+1]=c.y,u[h+2]=c.z,Math.floor(h/_)){case 0:v.set(1,0,0),g[M+0]=Yn(v,c,"z","y",s,i),g[M+1]=1-Yn(v,c,"y","z",s,t);break;case 1:v.set(-1,0,0),g[M+0]=1-Yn(v,c,"z","y",s,i),g[M+1]=1-Yn(v,c,"y","z",s,t);break;case 2:v.set(0,1,0),g[M+0]=1-Yn(v,c,"x","z",s,e),g[M+1]=Yn(v,c,"z","x",s,i);break;case 3:v.set(0,-1,0),g[M+0]=1-Yn(v,c,"x","z",s,e),g[M+1]=1-Yn(v,c,"z","x",s,i);break;case 4:v.set(0,0,1),g[M+0]=1-Yn(v,c,"x","y",s,e),g[M+1]=1-Yn(v,c,"y","x",s,t);break;case 5:v.set(0,0,-1),g[M+0]=Yn(v,c,"x","y",s,e),g[M+1]=1-Yn(v,c,"y","x",s,t);break}}static fromJSON(e){return new n(e.width,e.height,e.depth,e.segments,e.radius)}};function Yd(n,e=1e-4){e=Math.max(e,Number.EPSILON);let t={},i=n.getIndex(),r=n.getAttribute("position"),s=i?i.count:r.count,o=0,a=Object.keys(n.attributes),l={},c={},d=[],m=["getX","getY","getZ","getW"],u=["setX","setY","setZ","setW"];for(let M=0,P=a.length;M<P;M++){let w=a[M],T=n.attributes[w];l[w]=new T.constructor(new T.array.constructor(T.count*T.itemSize),T.itemSize,T.normalized);let E=n.morphAttributes[w];E&&(c[w]||(c[w]=[]),E.forEach((U,p)=>{let S=new U.array.constructor(U.count*U.itemSize);c[w][p]=new U.constructor(S,U.itemSize,U.normalized)}))}let g=e*.5,_=Math.log10(1/e),v=Math.pow(10,_),f=g*v;for(let M=0;M<s;M++){let P=i?i.getX(M):M,w="";for(let T=0,E=a.length;T<E;T++){let U=a[T],p=n.getAttribute(U),S=p.itemSize;for(let y=0;y<S;y++)w+=`${Math.trunc(p[m[y]](P)*v+f)},`}if(w in t)d.push(t[w]);else{for(let T=0,E=a.length;T<E;T++){let U=a[T],p=n.getAttribute(U),S=n.morphAttributes[U],y=p.itemSize,R=l[U],I=c[U];for(let k=0;k<y;k++){let D=m[k],N=u[k];if(R[N](o,p[D](P)),S)for(let H=0,$=S.length;H<$;H++)I[H][N](o,S[H][D](P))}}t[w]=o,d.push(o),o++}}let h=n.clone();for(let M in n.attributes){let P=l[M];if(h.setAttribute(M,new P.constructor(P.array.slice(0,o*P.itemSize),P.itemSize,P.normalized)),M in c)for(let w=0;w<c[M].length;w++){let T=c[M][w];h.morphAttributes[M][w]=new T.constructor(T.array.slice(0,o*T.itemSize),T.itemSize,T.normalized)}}return h.setIndex(d),h}var g_=new Ne(2827296),tr=null;function x_(){if(tr)return tr;let n=[90,170,255],e=new Uint8Array(n.length*4);return n.forEach((t,i)=>e.set([t,t,t,255],i*4)),tr=new fr(e,n.length,1,Fn),tr.minFilter=Qt,tr.magFilter=Qt,tr.generateMipmaps=!1,tr.needsUpdate=!0,tr}function Et(n={}){let{roughness:e,metalness:t,...i}=n;return new $s({gradientMap:x_(),...i})}var v_=`
uniform float thickness;
#include <common>
#include <fog_pars_vertex>
void main() {
  vec4 mvPosition = modelViewMatrix * vec4(position, 1.0);
  vec3 n = normalize(normalMatrix * normal);
  // A constant screen-ish width: thicker the farther away, so outlines read at every zoom.
  mvPosition.xyz += n * thickness * clamp(-mvPosition.z * 0.06, 0.7, 4.0);
  gl_Position = projectionMatrix * mvPosition;
  #include <fog_vertex>
}`,__=`
uniform vec3 color;
#include <common>
#include <fog_pars_fragment>
void main() {
  gl_FragColor = vec4(color, 1.0);
  #include <fog_fragment>
}`,Al=null;function y_(){return Al||(Al=new Tn({uniforms:xl.merge([Te.fog,{thickness:{value:.024},color:{value:g_}}]),vertexShader:v_,fragmentShader:__,side:fn,fog:!0}),Al)}var Zd=new WeakMap;function b_(n){let e=Zd.get(n);if(e)return e;let t=new $t;t.setAttribute("position",n.getAttribute("position")),n.index&&t.setIndex(n.index);let i=Yd(t,.001);return i.computeVertexNormals(),Zd.set(n,i),i}function Ci(n,e=.12){let t=[];n.traverse(i=>{let r=i;if(!r.isMesh||r.userData.outline===!1||r.userData.isOutline)return;let s=Array.isArray(r.material)?r.material[0]:r.material;if(!s||s.transparent||s.isMeshBasicMaterial)return;let o=r.geometry;o.boundingSphere||o.computeBoundingSphere(),!((o.boundingSphere?.radius??0)<e)&&t.push(r)});for(let i of t){let r=new rt(b_(i.geometry),y_());r.userData.isOutline=!0,r.castShadow=!1,r.receiveShadow=!1,r.raycast=()=>{},i.add(r)}}var M_=`
varying vec3 vDir;
void main() {
  vDir = normalize(position);
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  gl_Position.z = gl_Position.w; // always at the far plane
}`,S_=`
uniform vec3 top;
uniform vec3 horizon;
uniform vec3 ground;
varying vec3 vDir;
void main() {
  float h = vDir.y;
  // Hard-edged bands near the horizon, the way a painted backdrop does it.
  float t = smoothstep(0.0, 0.55, h);
  t = floor(t * 6.0 + 0.5) / 6.0;
  vec3 c = mix(horizon, top, t);
  c = mix(ground, c, smoothstep(-0.05, 0.02, h));
  gl_FragColor = vec4(c, 1.0);
}`;function Kd(n,e,t){let i=new Tn({uniforms:{top:{value:n},horizon:{value:e},ground:{value:t}},vertexShader:M_,fragmentShader:S_,side:fn,depthWrite:!1}),r=new rt(new Vt(400,32,16),i);return r.renderOrder=-1,r.frustumCulled=!1,r}function Jd(n,e,t){let i=new ht,r=Et({color:16777215,emissive:14542576,emissiveIntensity:.35}),s=new Vt(1,16,12);for(let o=0;o<n;o++){let a=new ht,l=4+Math.floor(t()*3);for(let m=0;m<l;m++){let u=1.6+t()*1.6,g=new rt(s,r);g.scale.set(u*1.3,u*.8,u),g.position.set((m-l/2)*1.9+t(),t()*.8,(t()-.5)*2),a.add(g)}let c=o/n*Math.PI*2+t()*.4,d=e*(.7+t()*.5);a.position.set(Math.cos(c)*d,26+t()*10,Math.sin(c)*d),a.rotation.y=t()*Math.PI,i.add(a)}return Ci(i,0),{group:i,animate:o=>i.rotation.y=o*.006}}function jd(n,e,t,i){let r=[];for(let _=0;_<3;_++){let v=_/3*Math.PI+.3,f=Math.cos(v)*.07,h=Math.sin(v)*.07,M=(_-1)*.06;r.push(-f,0,-h,f,0,h,M,.32+_*.05,0)}let s=new $t;s.setAttribute("position",new bt(r,3)),s.computeVertexNormals();let o=Et({color:16777215,side:Nn}),a={uTime:{value:0}};o.onBeforeCompile=_=>{_.uniforms.uTime=a.uTime,_.vertexShader=_.vertexShader.replace("#include <common>",`#include <common>
uniform float uTime;`).replace("#include <begin_vertex>",`#include <begin_vertex>
        vec4 wp = instanceMatrix * vec4(0.0, 0.0, 0.0, 1.0);
        float sway = sin(uTime * 1.7 + wp.x * 0.35 + wp.z * 0.25) * 0.5 + sin(uTime * 3.1 + wp.x) * 0.15;
        transformed.x += sway * position.y * 0.45;
        transformed.z += sway * position.y * 0.2;`),_.vertexShader=_.vertexShader.replace("#include <color_vertex>",`#include <color_vertex>
vTipShade = position.y;`),_.vertexShader=_.vertexShader.replace("#include <common>",`#include <common>
varying float vTipShade;`),_.fragmentShader=_.fragmentShader.replace("#include <common>",`#include <common>
varying float vTipShade;`).replace("#include <color_fragment>",`#include <color_fragment>
diffuseColor.rgb *= 0.75 + vTipShade * 1.1;`)};let l=new Bs(s,o,n),c=new wt,d=new an,m=new G(0,1,0),u=new Ne,g=0;for(let _=0;_<n*4&&g<n;_++){let v=(i()*2-1)*e,f=(i()*2-1)*e;if(Math.hypot(v,f)>e||!t(v,f))continue;let h=.7+i()*.9;d.setFromAxisAngle(m,i()*Math.PI*2),c.compose(new G(v,0,f),d,new G(h,h,h)),l.setMatrixAt(g,c),l.setColorAt(g,u.setHSL(.24+i()*.07,.5+i()*.2,.42+i()*.14)),g++}return l.count=g,l.receiveShadow=!0,l.instanceMatrix.needsUpdate=!0,l.instanceColor&&(l.instanceColor.needsUpdate=!0),l.userData.outline=!1,{mesh:l,animate:_=>a.uTime.value=_}}var go=.46,w_=.5,E_=.78,T_=.82,xo=.24,Qd=1.3,A_=1.25*Qd,R_=.35;function ef(n,e,t,i=n){let r=new ht;r.name=e?"robot-orchestrator":"robot",r.scale.setScalar(e?A_:Qd);let s=[],o=[],a=(pe,Pe,Ve=.1)=>{let Oe=Et({color:pe,roughness:Pe,metalness:Ve});return s.push(Oe),o.push(Oe),Oe},l=a(new Ne().setHSL(i/360,.6,.64,kt),.65),c=a(t.paper,.6),d=a(new Ne().setHSL(n/360,.5,.82,kt),.5),m=a(t.rule,.45),u=a(t.ink,.8,.05),g=a(t.signal,.5),_=Et({color:t.ink,roughness:.12,metalness:.4}),v=Et({color:t.ok,emissive:t.ok,emissiveIntensity:1.2,roughness:.3}),f=Et({color:t.signal,emissive:t.signal,emissiveIntensity:1.2,roughness:.4});s.push(_,v,f);let h=(pe,Pe,Ve,Oe=0,oe=0,le=0)=>{let be=new rt(Pe,Ve);return be.position.set(Oe,oe,le),be.castShadow=!0,be.receiveShadow=!0,pe.add(be),be},M=new ht;r.add(M);let P=new Ri(.16,.22,.86,3,.07);h(M,P,u,-.36,.11,0),h(M,P,u,.36,.11,0);let w=new zt(.08,.08,.03,16);w.rotateZ(Math.PI/2);let T=[];for(let pe of[-1,1])for(let Pe of[-.28,0,.28])T.push(h(M,w,m,pe*.445,.11,Pe));let E=new ht;r.add(E),h(E,new Ri(.62,w_,.6,4,.12),l,0,go,0),h(E,new Ri(.36,.26,.03,2,.01),d,0,go,.315),h(E,new Qe(.28,.012,.012),m,0,go,.33);let U=new Vt(.018,8,6);for(let pe of[-.15,.15])for(let Pe of[go-.1,go+.1])h(E,U,m,pe,Pe,.33);h(E,new zt(.07,.08,.14,12),u,0,E_,0);let p=new zt(.045,.04,.3,12);p.translate(0,-.15,0);let S=new Ri(.1,.05,.08,2,.02),y=new Qe(.025,.08,.03),R=pe=>{let Pe=new ht;Pe.position.set(pe*.34,.6,0),E.add(Pe),h(Pe,p,l);let Ve=new ht;return Ve.position.set(0,-.3,0),Pe.add(Ve),h(Ve,S,u),h(Ve,y,u,-.035,-.05,0),h(Ve,y,u,.035,-.05,0),Pe},I=R(-1),k=R(1),D=new ht;D.position.set(0,T_,0),E.add(D),h(D,new Ri(.62,xo,.42,3,.09),c,0,xo/2,0);let N=new zt(.13,.13,.18,24);N.rotateX(Math.PI/2);let H=new di(.1,24),$=new di(.07,24),ee=[];for(let pe of[-.16,.16])h(D,N,c,pe,.12,.2),h(D,H,_,pe,.12,.292),ee.push(h(D,$,v,pe,.12,.296));let J;e&&(h(D,new zt(.01,.01,.16,6),u,.22,xo+.08,-.05),J=h(D,new Vt(.03,10,8),f,.22,xo+.17,-.05),h(E,new Ri(.36,.03,.02,2,.008),g,0,.62,.31));let ie="idle",ce=1.2,we=pe=>{ie=pe;let Pe=pe==="offline"?t.inkMuted:pe==="working"?t.info:t.ok;v.color.set(Pe),v.emissive.set(Pe),ce=pe==="offline"?.2:pe==="working"?2:1.3};return we("idle"),Ci(r,.03),{group:r,labelAnchor(pe){return D.updateWorldMatrix(!0,!1),D.localToWorld(pe.set(0,xo+R_,0))},setState(pe){pe!==ie&&we(pe)},update(pe,Pe){let Ve=Math.min(1,pe*8),Oe=ie,oe=0,le=0;Oe==="idle"?(oe=.03*Math.sin(Pe*.5),le=.12*Math.sin(Pe*.9)):Oe==="working"?(oe=.14+.08*Math.sin(Pe*5),le=.04*Math.sin(Pe*1.3)):Oe==="walking"?(oe=.04,le=.06*Math.sin(Pe*3)):(oe=.5,le=.12),D.rotation.x+=(oe-D.rotation.x)*Ve,D.rotation.z+=(le-D.rotation.z)*Ve;let be=0,Ge=0;if(Oe==="idle"?be=.012*Math.sin(Pe*1.6):Oe==="working"?be=.004*Math.sin(Pe*2.5):Oe==="walking"?(be=.01*Math.abs(Math.sin(Pe*6)),Ge=.05*Math.sin(Pe*6)):be=-.02,E.position.y=be,E.rotation.z=Ge,M.position.y=Oe==="walking"?.012*Math.abs(Math.sin(Pe*10)):0,Oe==="walking")for(let ze of T)ze.rotation.x+=pe*8;if(Oe==="working"){let ze=Math.sin(Pe*11);I.rotation.x=-.9+.2*ze,k.rotation.x=-.9-.2*ze}else if(Oe==="walking"){let ze=.25*Math.sin(Pe*6);I.rotation.x=ze,k.rotation.x=-ze}else if(Oe==="idle"){let ze=.05*Math.sin(Pe*1.1);I.rotation.x=ze,k.rotation.x=-ze}else I.rotation.x=.2,k.rotation.x=.2;let Me=1;Oe==="idle"&&Pe%3.6<.14?Me=.1:Oe==="offline"&&(Me=.45);for(let ze of ee)ze.scale.y=Me;v.emissiveIntensity=Oe==="working"?ce+.3*Math.sin(Pe*5):ce,J&&(f.emissiveIntensity=1.1+.5*Math.sin(Pe*3))},setHighlighted(pe){for(let Pe of o)Pe.emissive.set(pe?t.signal:0),Pe.emissiveIntensity=pe?.35:0},dispose(){r.traverse(pe=>{pe instanceof rt&&pe.geometry.dispose()});for(let pe of s)pe.dispose()}}}var tf=new Map,nf=new Map;function ps(n){let e=n>>>0;return()=>{e=e+1831565813>>>0;let t=e;return t=Math.imul(t^t>>>15,t|1),t^=t+Math.imul(t^t>>>7,t|61),((t^t>>>14)>>>0)/4294967296}}function C_(n){let e=2166136261;for(let t=0;t<n.length;t++)e=Math.imul(e^n.charCodeAt(t),16777619);return e>>>0}function gi(n,e,t,i,r){let s=tf.get(n);if(s)return s;let o=document.createElement("canvas");o.width=e,o.height=t;let a=o.getContext("2d");a&&r(a,e,t,ps(C_(n)));let l=new pr(o);return l.wrapS=dr,l.wrapT=dr,l.repeat.set(i,i),l.colorSpace=kt,l.anisotropy=4,tf.set(n,l),l}var yr=n=>n.getStyle();function Rl(n,e,t,i,r,s){n.beginPath(),n.moveTo(e+s,t),n.arcTo(e+i,t,e+i,t+r,s),n.arcTo(e+i,t+r,e,t+r,s),n.arcTo(e,t+r,e,t,s),n.arcTo(e,t,e+i,t,s),n.closePath()}function vu(){return gi("grass",512,512,24,(n,e,t,i)=>{n.fillStyle="#86c45e",n.fillRect(0,0,e,t);for(let r=0;r<14;r++)n.fillStyle=i()<.5?"#8fcb66":"#7fbc58",n.beginPath(),n.ellipse(i()*e,i()*t,30+i()*60,20+i()*40,i()*Math.PI,0,Math.PI*2),n.fill();n.lineCap="round";for(let r=0;r<90;r++){let s=i()*e,o=i()*t,a=4+i()*5;n.strokeStyle=i()<.7?"rgba(70,128,52,0.55)":"rgba(196,232,150,0.7)",n.lineWidth=1.6,n.beginPath(),n.moveTo(s-a*.6,o-a),n.lineTo(s,o),n.lineTo(s+a*.1,o-a*1.3),n.moveTo(s,o),n.lineTo(s+a*.7,o-a*.9),n.stroke()}for(let r=0;r<12;r++)n.fillStyle=["#fff6d8","#ffd25e","#ff9db5"][Math.floor(i()*3)],n.beginPath(),n.arc(i()*e,i()*t,2+i()*1.5,0,Math.PI*2),n.fill()})}function vo(n,e=4){return gi(`paving:${yr(n)}:${e}`,256,256,e,(t,i,r,s)=>{let a=r/5,l=i/4;t.fillStyle=yr(n.clone().multiplyScalar(.55)),t.fillRect(0,0,i,r);for(let c=0;c<5;c++){let d=c%2*(l/2);for(let m=-1;m<=4;m++){let u=m*l+d+3+s()*2,g=c*a+3+s()*2,_=l-7,v=a-7;Rl(t,u,g,_,v,9),t.fillStyle=yr(n.clone().multiplyScalar(.92+s()*.14)),t.fill(),t.lineWidth=2,t.strokeStyle="rgba(43,36,32,0.55)",t.stroke(),Rl(t,u+5,g+4,_*.45,5,2.5),t.fillStyle="rgba(255,255,255,0.45)",t.fill()}}})}function rf(n,e=3){return gi(`tiles:${Math.round(n)}:${e}`,256,256,e,(t,i,r,s)=>{let o=new Ne().setHSL(n/360,.28,.66,kt),a=new Ne().setHSL(n/360,.5,.84,kt);t.fillStyle=yr(o),t.fillRect(0,0,i,r);let l=4,c=i/l;for(let d=0;d<l;d++)for(let m=0;m<l;m++){let u=(m+d)%2===0?1:.9;Rl(t,m*c+3,d*c+3,c-6,c-6,6),t.fillStyle=yr(a.clone().multiplyScalar(u*(.97+s()*.06))),t.fill(),t.beginPath(),t.moveTo(m*c+10,d*c+14),t.lineTo(m*c+22,d*c+9),t.strokeStyle="rgba(255,255,255,0.6)",t.lineWidth=3,t.lineCap="round",t.stroke()}})}function _o(){return gi("wood",256,256,1,(n,e,t,i)=>{let s=t/4;for(let o=0;o<4;o++){n.fillStyle=`hsl(${28+i()*6}, ${55+i()*10}%, ${50+i()*8}%)`,n.fillRect(0,o*s,e,s),n.fillStyle="rgba(255,255,255,0.18)",n.fillRect(0,o*s+4,e,5);for(let a=0;a<2;a++)n.strokeStyle="rgba(90,50,20,0.45)",n.lineWidth=2,n.beginPath(),n.ellipse(i()*e,o*s+s/2,7+i()*5,3,0,0,Math.PI*2),n.stroke();n.fillStyle="rgba(60,32,14,0.8)",n.fillRect(0,o*s,e,3)}})}function _u(){return gi("metal",64,64,1,(n,e,t)=>{n.fillStyle="#8f9bab",n.fillRect(0,0,e,t),n.fillStyle="rgba(255,255,255,0.35)",n.fillRect(0,t*.2,e,t*.12)})}function sf(n){return gi(`polish:${yr(n)}`,256,256,2,(e,t,i,r)=>{e.fillStyle=yr(n),e.fillRect(0,0,t,i);for(let s=0;s<14;s++)e.fillStyle="rgba(255,255,255,0.35)",Rl(e,r()*t,r()*i,18+r()*30,5,2.5),e.fill();for(let s=0;s<10;s++)e.fillStyle="rgba(120,100,80,0.12)",e.beginPath(),e.arc(r()*t,r()*i,8+r()*14,0,Math.PI*2),e.fill()})}function of(){return gi("water",256,256,2,(n,e,t,i)=>{n.fillStyle="#3cc0d6",n.fillRect(0,0,e,t),n.fillStyle="#5fd3e3";for(let r=0;r<10;r++)n.beginPath(),n.ellipse(i()*e,i()*t,30+i()*30,12+i()*10,0,0,Math.PI*2),n.fill();n.lineCap="round";for(let r=0;r<26;r++){let s=i()*e,o=i()*t;n.strokeStyle="rgba(255,255,255,0.85)",n.lineWidth=3,n.beginPath(),n.moveTo(s,o),n.quadraticCurveTo(s+8,o-6,s+16,o),n.quadraticCurveTo(s+24,o+6,s+32,o),n.stroke()}})}function yu(n){let e=`side:${Math.round(n)}`,t=nf.get(e);if(t)return t;let i=Et({color:new Ne().setHSL(n/360,.35,.66,kt)});return nf.set(e,i),i}var P_=new Set(["builder","qa","release","security"]),I_=new Set(["researcher","librarian","planner"]),L_=new Set(["architect","reviewer"]),Mu=new Ne(16766874),cf=new Ne(13208422);function Yt(n,e,t=.1){return Et({color:n,roughness:e,metalness:t})}function si(n,e,t){let i=new Ne().setHSL(e/360,.45,.6,kt);return n.clone().lerp(i,t)}function qe(n,e,t,i,r,s){let o=new rt(e,t);return o.position.set(i,r,s),n.add(o),o}function Mr(n){n.traverse(e=>{e instanceof rt&&(e.castShadow=!0,e.receiveShadow=!0)})}function br(n){n.updateWorldMatrix(!0,!1);let e=new Dn().setFromObject(n);return{x0:e.min.x,x1:e.max.x,z0:e.min.z,z1:e.max.z}}function bu(n,e){return n[Math.floor(e()*n.length)]}function af(n,e){return Math.abs(Math.atan2(Math.sin(n-e),Math.cos(n-e)))}function k_(n,e,t,i){let r=Math.max(t*t+i*i,1e-6),s=Math.max(0,Math.min(1,(n*t+e*i)/r));return Math.hypot(n-t*s,e-i*s)}function uf(n){let e=new ht;return qe(e,new Qe(1.3,.08,.42),n,0,.42,0),qe(e,new Qe(.08,.4,.36),n,-.55,.2,0),qe(e,new Qe(.08,.4,.36),n,.55,.2,0),qe(e,new Qe(1.3,.4,.07),n,0,.66,-.2),e}function D_(n,e,t){let i=gi(`sign:${n}:${e.getStyle()}:${t.getStyle()}`,256,96,1,(r,s,o)=>{r.fillStyle=e.getStyle(),r.fillRect(0,0,s,o),r.strokeStyle=t.getStyle(),r.lineWidth=6,r.strokeRect(6,6,s-12,o-12),r.fillStyle=t.getStyle(),r.font="600 30px ui-monospace, SFMono-Regular, Menlo, monospace",r.textAlign="center",r.textBaseline="middle",r.fillText(n,s/2,o/2+2)});return Et({map:i,roughness:.6})}function N_(n,e,t,i,r,s){let o=new ht;o.position.set(e,0,t),qe(o,new Qe(1,.06,.55),i,0,.55,0),qe(o,new Qe(.08,.55,.5),i,-.42,.275,0),qe(o,new Qe(.08,.55,.5),i,.42,.275,0),qe(o,new Qe(.3,.02,.18),r,0,.59,-.12),qe(o,new Qe(.07,.2,.05),r,0,.7,-.12),qe(o,new Qe(.62,.38,.04),s,0,.95,-.14),n.add(o)}function lf(n,e,t,i,r,s,o){qe(n,new zt(.22*i,.16*i,.3*i,14),r,e,.15*i,t),qe(n,new Vt(.34*i,14,10),s,e,.5*i,t),qe(n,new es(.22*i,.45*i,10),o,e,.85*i,t)}function hf(n,e,t,i){let r=new ht,s=e,o=Yt(si(i.rule,n,.3),.8,0),a=Yt(si(i.ink3,n,.2),.6,.2),l=Yt(si(i.ink3,n,.1),.45,.4),c=Yt(si(cf,n,.1),.9,0),d=Yt(si(i.ok,n,.2),.85,0),m=Yt(si(i.ok,n,.45),.85,0),u=Yt(si(i.rule,n,.55),.9,0),g=Et({color:si(i.ink3,n,.3),emissive:i.info,emissiveIntensity:.05,roughness:.35}),_=new ln({color:i.ok,toneMapped:!1}),v=new ln({color:i.info,toneMapped:!1}),f=s>=3?3:2;for(let S=0;S<f;S++)N_(r,-s+1.2+S*(2*s-2.4)/(f-1),s-.9,o,l,g);lf(r,s-.5,-s+.55,1,c,d,m),s>=3&&lf(r,-s+.5,-s+.55,.9,c,d,m),qe(r,new Qe(.5,.5,.5),u,s-.5,.25,.05),qe(r,new Qe(.45,.45,.45),u,s-.45,.725,.1);for(let S of[s-1.6,-s+1.6])qe(r,new Qe(.3,.4,1.2),d,S,.2,0);let h=-s+.4,M=s-.4;qe(r,new zt(.035,.035,1.9,8),l,h,.95,M);let P=new ln({color:Mu,toneMapped:!1});qe(r,new Vt(.13,12,8),P,h,1.95,M);let w=new js(Mu,.35,4.5,2);w.position.set(h,1.8,M),r.add(w);let T=-s+.5;if(P_.has(t))for(let S of[-.55,.2]){qe(r,new Qe(.5,1.5,.45),a,T,.75,S);for(let y of[.3,.6,.9,1.2])for(let R of[-.12,.12])qe(r,new Qe(.02,.05,.05),y+R>.7?_:v,T+.26,y,S+R)}else if(I_.has(t)){let S=[si(i.signal,n,.3),si(i.info,n,.2),si(i.ok,n,.1)].map(y=>Yt(y,.7,0));qe(r,new Qe(.5,1.5,1),a,T,.75,-.2);for(let y of[.35,.75,1.15]){qe(r,new Qe(.56,.04,1),o,T,y,-.2);for(let R=0;R<4;R++)qe(r,new Qe(.02,.22,.12),S[R%S.length]??o,T+.265,y+.13,-.6+R*.28)}}else if(L_.has(t)){let S=Yt(i.sheet,.5,0);qe(r,new Qe(.08,1,1.5),l,-s+.1,1.05,0),qe(r,new Qe(.02,.8,1.3),S,-s+.15,1.05,0),qe(r,new Qe(.02,.12,.4),Yt(i.signal,.6,0),-s+.17,1.2,-.25),qe(r,new Qe(.02,.1,.5),Yt(i.info,.6,0),-s+.17,.9,.3)}else qe(r,new Qe(.4,.35,1.3),c,T,.175,-.1),qe(r,new Vt(.3,12,8),d,T,.55,-.5),qe(r,new Vt(.3,12,8),m,T,.55,.3);let E=D_(t.toUpperCase(),i.sheet,i.ink3),U=new ht;U.position.set(0,0,-s+1.5),qe(U,new zt(.035,.035,1.2,8),l,0,.6,0);let p=new rt(new Qe(.9,.34,.04),[o,o,o,o,E,E]);return p.position.set(0,1.05,0),U.add(p),r.add(U),r.userData={screens:[g],animate:S=>{_.color.copy(i.ok).multiplyScalar(.35+.65*Math.max(0,Math.sin(S*2.7))),v.color.copy(i.info).multiplyScalar(.35+.65*Math.max(0,Math.sin(S*3.9+1))),w.intensity=.35*(1+.03*Math.sin(S*7.1)+.02*Math.sin(S*2.3))}},Mr(r),r}function df(n){let e=new ht,t=n.rule.clone().lerp(n.sheet,.4),i=qe(e,new zt(4.3,4.45,.06,64),Et({map:vo(t,4),roughness:.9}),0,.03,0);i.receiveShadow=!0;let r=qe(e,new qs(3.7,3.95,72),Yt(t.clone().multiplyScalar(.72),.9,0),0,.066,0);r.rotation.x=-Math.PI/2;let s=qe(e,new Wi(3.3,.025,6,72),Yt(n.rule,.8,0),0,.065,0);s.rotation.x=Math.PI/2;let o=Et({map:_o(),color:n.rule.clone().lerp(n.ink3,.25),roughness:.8});for(let d=0;d<4;d++){let m=d/4*Math.PI*2,u=Math.cos(m)*3.6,g=Math.sin(m)*3.6,_=uf(o);_.position.set(u,0,g),_.rotation.y=Math.atan2(-u,-g),e.add(_)}let a=new ht,l=new rt(new Xs(.5,0),Et({color:n.signal,emissive:n.signal,emissiveIntensity:.25}));l.scale.set(1,1.35,1);let c=new rt(new Wi(.85,.05,8,48),Et({color:n.sheet}));return c.rotation.x=Math.PI/2.4,a.add(l,c),a.position.set(0,3.7,0),e.add(a),e.userData={animate:d=>{a.rotation.y=d*.4,a.position.y=3.7+Math.sin(d*.9)*.08}},Mr(e),a.castShadow=!1,Ci(e),e}function ff(n,e){let t=new ht,i=[],r=4,s=[];for(let _=0;_<8;_++){let v=Math.PI/8+_*Math.PI/4;e.some(f=>af(v,f)<.3)||s.every(f=>af(v,f)>1)&&s.push(v)}let o=of(),a=Et({map:sf(n.sheet),roughness:.4}),l=Et({map:o,roughness:.12,transparent:!0,opacity:.9}),c=Et({map:_o(),roughness:.8}),d=Et({map:_u(),roughness:.4,metalness:.5}),m=gi(`awning:${n.signal.getStyle()}:${n.sheet.getStyle()}`,256,64,1,(_,v,f)=>{for(let M=0;M<8;M++)_.fillStyle=M%2===1?n.signal.getStyle():n.sheet.getStyle(),_.fillRect(M*v/8,0,v/8,f)}),u=Et({map:m,roughness:.9,side:Nn});s.forEach((_,v)=>{let f=Math.cos(_)*r,h=Math.sin(_)*r,M=new ht;if(M.position.set(f,0,h),v===0){let P=qe(M,new zt(.62,.7,.26,32),a,0,.13,0),w=qe(M,new di(.55,32),l,0,.27,0);w.rotation.x=-Math.PI/2,qe(M,new zt(.05,.07,.6,8),a,0,.56,0),qe(M,new Vt(.11,12,8),l,0,.92,0),i.push(br(P))}else{M.rotation.y=Math.atan2(-f,-h);let P=qe(M,new Qe(1.1,.72,.5),c,0,.36,0);qe(M,new Qe(.26,.18,.26),Yt(n.ok,.7,0),.25,.81,0),qe(M,new Qe(.24,.16,.24),Yt(n.signal,.7,0),-.3,.8,.05);for(let T of[-.5,.5])qe(M,new zt(.03,.03,2,6),d,T,1,.42);let w=qe(M,new Qe(1.4,.04,.95),u,0,2,.1);w.rotation.x=.2,i.push(br(P))}Mr(M),t.add(M)});let g=_=>{o.offset.y=_*.04%1};return Ci(t),{group:t,boxes:i,animate:g}}function pf(n,e){let t=new ht,i=Et({map:vo(e.rule.clone().lerp(e.sheet,.35),2),roughness:.95});for(let r of n){let s=Math.hypot(r.x,r.z);if(s<.5)continue;let o=new Qe(s,.03,.9),a=o.getAttribute("uv");for(let c=0;c<a.count;c++)a.setX(c,a.getX(c)*(s/.9));let l=new rt(o,i);l.position.set(r.x/2,.015,r.z/2),l.rotation.y=-Math.atan2(r.z,r.x),l.receiveShadow=!0,t.add(l)}return t}function mf(n,e){let t=new ht,i=[],r=ps(1177),s=0;for(let S of n)s=Math.max(s,Math.hypot(S.cx,S.cy)+S.r);let o=Math.min(34,s+6),a=Et({map:_o(),roughness:.9}),l=[6262602,7840346,5077564,9416286].map(S=>Yt(new Ne(S),.85,0)),c=[3104058,3960391].map(S=>Yt(new Ne(S),.9,0)),d=[15907014,16245738].map(S=>Yt(new Ne(S),.85,0)),m=[15909198,15305376,16249832,14243644,10190801].map(S=>Yt(new Ne(S),.7,0)),u=Yt(e.rule.clone().lerp(e.ink3,.6),1,0),g=Yt(cf,.9,0),_=Yt(e.ink3.clone().lerp(e.sheet,.45),.95,0),v=Et({map:_o(),roughness:.8}),f=Et({map:_u(),roughness:.4,metalness:.5}),h=new ln({color:Mu,toneMapped:!1}),M=(S,y,R)=>{if(Math.hypot(S,y)<6.4+R)return!1;for(let I of n)if(Math.abs(S-I.cx)<I.r+3+R&&Math.abs(y-I.cy)<I.r+3+R||k_(S,y,I.cx,I.cy)<1.4+R)return!1;for(let I of i)if(S+R>I.x0-.3&&S-R<I.x1+.3&&y+R>I.z0-.3&&y-R<I.z1+.3)return!1;return!0},P=[],w=(S,y)=>{if(P.some(([k,D])=>Math.hypot(k-S,D-y)<3))return;P.push([S,y]);let R=new ht;R.position.set(S,0,y);let I=qe(R,new zt(.035,.045,2.2,8),f,0,1.1,0);qe(R,new zt(.06,.08,.1,10),f,0,2.25,0),qe(R,new Vt(.16,14,10),h,0,2.4,0),t.add(R),Mr(R),i.push(br(I))},T=(S,y)=>{let R=new ht;R.position.set(S,0,y);let I=.9+r()*.5,k=qe(R,new zt(.08,.12,I,8),a,0,I/2,0);if(r()<.35){let D=bu(c,r);for(let N=0;N<3;N++)qe(R,new es(.8-N*.2,.95,10),D,0,I+.25+N*.42,0)}else{let D=bu(r()<.2?d:l,r);qe(R,new Vt(.72,14,10),D,0,I+.35,0),qe(R,new Vt(.5,12,8),D,.28,I+.8,.12),qe(R,new Vt(.42,12,8),D,-.22,I+.7,-.18)}t.add(R),Mr(R),i.push(br(k))},E=(S,y)=>{let R=new ht;R.position.set(S,0,y),R.rotation.y=r()*Math.PI;let I=qe(R,new Qe(1.5,.2,.9),g,0,.1,0);qe(R,new Qe(1.36,.06,.76),u,0,.19,0);for(let k=0;k<16;k++){let D=bu(m,r);qe(R,new Vt(.06+r()*.03,8,6),D,(r()-.5)*1.1,.3+r()*.06,(r()-.5)*.6)}t.add(R),Mr(R),i.push(br(I))},U=(S,y)=>{let R=.3+r()*.35,I=new rt(new Ws(R,0),_);I.position.set(S,R*.4,y),I.rotation.set(r()*3,r()*3,r()*3),I.castShadow=!0,I.receiveShadow=!0,t.add(I),i.push(br(I))},p=(S,y)=>{let R=uf(v);R.position.set(S,0,y),R.rotation.y=r()*Math.PI*2,t.add(R),Mr(R),i.push(br(R))};n.forEach((S,y)=>{let R=Math.hypot(S.cx,S.cy),I=R-S.r-2.6;if(R<1||I<7)return;let k=S.cx/R,D=S.cy/R,N=y%2===0?1:-1;w(k*I-D*N*.95,D*I+k*N*.95)});for(let S=0;S<320;S++){let y=(r()*2-1)*o,R=(r()*2-1)*o,I=r();I<.42?M(y,R,.7)&&T(y,R):I<.62?M(y,R,.85)&&E(y,R):I<.84?M(y,R,.4)&&U(y,R):M(y,R,.7)&&p(y,R)}return Ci(t,.1),{group:t,boxes:i}}var U_=64,F_=32,O_=4e3,B_=1500,z_=1.6,en=10,gf=24,H_=60,xf=2,G_=.9,V_=[0,30,-30,60,-60];function Cl(n){return 2*(Math.max(1,Math.ceil(Math.sqrt(n.length)))+1)}function W_(n){let e=0;for(let t of n)e=e*31+t.charCodeAt(0)>>>0;return e%360}function vf(n,e,t=0){return[(n-e)*(U_/2),(n+e)*(F_/2)-t]}function X_(n){let e=new Map;for(let d of n){let m=d.tier==="orchestrator"?"":d.role;e.set(m,[...e.get(m)??[],d])}let t=[],i=new Map,r=(d,m)=>{t.push(d);let u=Math.max(1,Math.ceil(Math.sqrt(m.length)));m.sort((g,_)=>g.cid-_.cid).forEach((g,_)=>{let v=d.cx-(u-1)*2+_%u*4,f=d.cy-(u-1)*2+Math.floor(_/u)*4;i.set(g.cid,[d,v,f])})},s=e.get("");s&&r({role:"orchestrator",cx:0,cy:0,r:Cl(s),hue:25},s);let o=[...e.keys()].filter(d=>d).sort(),a=Math.max(0,...o.map(d=>Cl(e.get(d)))),l=s?Cl(s):0,c=Math.max(o.length>1?(2*a*Math.SQRT2+xf)/(2*Math.sin(Math.PI/o.length)):0,(l+a)*Math.SQRT2+xf);return o.forEach((d,m)=>{let u=e.get(d),g=m/o.length*Math.PI*2-Math.PI/2;r({role:d,cx:Math.round(Math.cos(g)*c),cy:Math.round(Math.sin(g)*c),r:Cl(u),hue:W_(d)},u)}),{districts:t,homes:i}}function q_(){let n=getComputedStyle(document.documentElement),e=(t,i)=>n.getPropertyValue(`--${t}`).trim()||i;return{paper:e("paper","#f5f3ee"),sheet:e("sheet","#ffffff"),rule:e("rule","#d6d2c8"),ink:e("ink","#1a1a1a"),ink2:e("ink-2","#4a4a4a"),ink3:e("ink-3","#8a8a8a"),signal:e("signal","#d9482b"),ok:e("ok","#2f8f5b"),info:e("info","#2f6fb0"),danger:e("danger","#c0392b")}}function Pi(n){return typeof n=="string"?n:""}function Sr(n,e,t){return Math.min(t,Math.max(e,n))}var $_=[[2.6,2.6],[-2.6,-2.6],[2.6,-2.6],[-2.6,2.6]],Y_={builder:"rack",qa:"rack",release:"rack",security:"rack",researcher:"shelf",librarian:"shelf",planner:"shelf",architect:"board",reviewer:"board"};function Z_(n){let e=$_.map(([t,i])=>({kind:"bench",x:t,y:i}));for(let t of n){if(t.role==="orchestrator")continue;let i=t.r-.7;e.push({kind:"plant",x:t.cx-i,y:t.cy-i,d:t},{kind:"desk",x:t.cx+i,y:t.cy-i,d:t},{kind:"desk",x:t.cx-i,y:t.cy+i,d:t},{kind:"lamp",x:t.cx+i,y:t.cy+i,d:t},{kind:"crate",x:t.cx-i,y:t.cy,d:t},{kind:Y_[t.role]??"crate",x:t.cx,y:t.cy-i,d:t}),t.r>=4&&e.push({kind:"desk",x:t.cx+i,y:t.cy,d:t})}return e}function K_(n,e,t,i){return i.some(r=>r!==n&&Math.hypot(r.x-e,r.y-t)<G_)}function J_(n,e){let t=e.filter(i=>i!==n&&i.district===n.district&&i.agent.status==="idle"&&Math.hypot(i.x-n.x,i.y-n.y)<8);return t[Math.floor(Math.random()*t.length)]??null}function _f(n){let e=x("canvas.floor-canvas",{"aria-label":"The plaza floor"}),t=x("div.floor-card",{hidden:!0}),i=x("div.floor-legend",null,x("span",null,x("i.floor-key.working"),"Working"),x("span",null,x("i.floor-key.idle"),"Idle"),x("span",null,x("i.floor-key.offline"),"Offline")),r=x("div.floor-stage",null,e,t,i);n.classList.add("floor-view"),n.append(x("div.view-head",null,x("h1",null,"Floor"),x("div.actions",null,Ye("Reset view",()=>R(),{kind:"ghost",icon:"refresh"}))),r);let s=e.getContext("2d"),o=[],a=[],l=[],c=new Map,d=[],m={x:0,y:0},u=1,g=0,_=0,v=1,f=null,h=null,M="",P=null,w=!1,T=!0,E=0,U=performance.now(),p=q_();function S(C){let L=X_(C);o=L.districts;let O=[];for(let z of C){let X=L.homes.get(z.cid);if(!X)continue;let[Z,re,fe]=X,me=c.get(z.cid);O.push(me?Object.assign(me,{agent:z,district:Z,home:[re,fe]}):{agent:z,district:Z,home:[re,fe],x:re,y:fe,tx:re,ty:fe,facing:1,walk:0,moving:!1,nextWander:performance.now()+Math.random()*3e3,bubble:null,activity:"idle",talk:null,talkUntil:0})}a=O,l=Z_(o),c.clear(),a.forEach(z=>c.set(z.agent.cid,z)),h&&(h=c.get(h.agent.cid)??null),f&&(f=c.get(f.agent.cid)??null),W()}function y(C,L,O=0){let z=g/2+m.x,X=_/2+m.y,[Z,re]=vf(C,L,O);return[z+Z*u,X+re*u]}function R(){if(!o.length){m={x:0,y:0},u=1;return}let C=1/0,L=-1/0,O=1/0,z=-1/0;for(let re of o){let fe=[[re.cx-re.r,re.cy-re.r],[re.cx+re.r,re.cy-re.r],[re.cx+re.r,re.cy+re.r],[re.cx-re.r,re.cy+re.r]];for(let[me,B]of fe){let[ue,se]=vf(me,B);C=Math.min(C,ue),L=Math.max(L,ue),O=Math.min(O,se-60),z=Math.max(z,se+en)}}let X=Math.max(1,L-C),Z=Math.max(1,z-O);u=Sr(Math.min(g*.85/X,_*.85/Z),.4,2.5),m={x:-((C+L)/2)*u,y:-((O+z)/2)*u}}function I(C,L){for(let O of a){let z=O.agent.status==="idle";if(z||(O.talk=null),z&&L>=O.nextWander){let Rt=Math.random()<.3?J_(O,a):null;if(Rt)O.talk=Rt.agent.cid,O.talkUntil=L+9e3,O.nextWander=L+9e3;else{let on=Math.max(.5,O.district.r-.7);O.tx=O.district.cx+(Math.random()*2-1)*on,O.ty=O.district.cy+(Math.random()*2-1)*on,O.nextWander=L+3e3+Math.random()*4e3}}let X=z&&O.talk!==null&&L<O.talkUntil?c.get(O.talk):void 0;O.talk!==null&&!X&&(O.talk=null);let[Z,re]=O.home,fe=X?[X.x+.9,X.y+.9]:z?[O.tx,O.ty]:[Z,re],[me,B]=fe,ue=me-O.x,se=B-O.y,ge=Math.hypot(ue,se),_e=Math.min(z_*C,ge),he=Math.atan2(se,ue),Ce=V_.map(Rt=>he+Rt*Math.PI/180).find(Rt=>!K_(O,O.x+Math.cos(Rt)*_e,O.y+Math.sin(Rt)*_e,a));O.moving=ge>.02&&Ce!==void 0,Ce!==void 0&&(O.x+=Math.cos(Ce)*_e,O.y+=Math.sin(Ce)*_e);let{cx:Le,cy:_t,r:ot}=O.district;if(O.x=Sr(O.x,Le-ot+.5,Le+ot-.5),O.y=Sr(O.y,_t-ot+.5,_t+ot-.5),O.moving){O.walk+=C*9;let Rt=ue-se;Math.abs(Rt)>.05&&(O.facing=Rt>0?1:-1)}X&&!O.moving&&(O.facing=X.x-O.x-(X.y-O.y)>=0?1:-1,O.bubble={text:"\xB7 \xB7 \xB7",until:L+3e3,tone:"info"},O.talk=null,O.nextWander=L+2500+Math.random()*2e3)}}function k(C,L){s.beginPath(),C.forEach(([O,z],X)=>X===0?s.moveTo(O,z):s.lineTo(O,z)),s.closePath(),s.fillStyle=L,s.fill()}function D(C,L,O,z,X){s.beginPath(),s.moveTo(C+X,L),s.arcTo(C+O,L,C+O,L+z,X),s.arcTo(C+O,L+z,C,L+z,X),s.arcTo(C,L+z,C,L,X),s.arcTo(C,L,C+O,L,X),s.closePath()}function N(C){let L=C.cx-C.r,O=C.cx+C.r,z=C.cy-C.r,X=C.cy+C.r,Z=(he,Ce)=>y(he,Ce,en),re=(he,Ce)=>y(he,Ce,0);k([Z(O,z),Z(O,X),re(O,X),re(O,z)],`hsl(${C.hue} 30% 32% / 0.5)`),k([Z(O,X),Z(L,X),re(L,X),re(O,X)],`hsl(${C.hue} 30% 22% / 0.6)`);let fe=[Z(L,z),Z(O,z),Z(O,X),Z(L,X)];k(fe,p.sheet),k(fe,`hsl(${C.hue} 30% 50% / 0.12)`);let me=C.role==="orchestrator";s.beginPath(),fe.forEach(([he,Ce],Le)=>Le===0?s.moveTo(he,Ce):s.lineTo(he,Ce)),s.closePath(),s.strokeStyle=me?p.signal:p.rule,s.lineWidth=me?2:1,s.stroke();let[B,ue]=Z(L,z),se=C.role.toUpperCase();s.font="10px ui-monospace, SFMono-Regular, Menlo, monospace";let ge=s.measureText(se).width+12,_e=ue+10;D(B-ge/2,_e,ge,16,3),s.fillStyle=p.sheet,s.fill(),s.strokeStyle=me?p.signal:p.rule,s.lineWidth=1,s.stroke(),s.fillStyle=p.ink2,s.textAlign="center",s.textBaseline="middle",s.fillText(se,B,_e+8.5)}function H(C,L,O){let Z=(ge,_e)=>y(ge,_e,en+12),re=(ge,_e)=>y(ge,_e,en),fe=[C-.32,L-.32],me=[C+.32,L-.32],B=[C+.32,L+.32],ue=[C-.32,L+.32];k([Z(me[0],me[1]),Z(B[0],B[1]),re(B[0],B[1]),re(me[0],me[1])],p.ink3),k([Z(B[0],B[1]),Z(ue[0],ue[1]),re(ue[0],ue[1]),re(B[0],B[1])],p.ink2),k([Z(fe[0],fe[1]),Z(me[0],me[1]),Z(B[0],B[1]),Z(ue[0],ue[1])],p.sheet);let se=[Z(C-.2,L-.2),Z(C+.2,L-.2),Z(C+.2,L+.2),Z(C-.2,L+.2)];s.save(),s.globalAlpha=O?1:.6,k(se,O?p.info:p.rule),s.restore()}function $(){s.save(),s.lineCap="round";for(let z of o){if(z.role==="orchestrator")continue;let[X,Z]=y(0,0),[re,fe]=y(z.cx,z.cy);s.beginPath(),s.moveTo(X,Z),s.lineTo(re,fe),s.globalAlpha=.8,s.strokeStyle=p.rule,s.lineWidth=11*u,s.stroke(),s.globalAlpha=.9,s.strokeStyle=p.sheet,s.lineWidth=6*u,s.stroke()}s.restore();let C=3.5,L=[y(-C,-C),y(C,-C),y(C,C),y(-C,C)];k(L,p.sheet),s.save(),s.strokeStyle=p.rule,s.lineWidth=1,s.beginPath(),L.forEach(([z,X],Z)=>Z===0?s.moveTo(z,X):s.lineTo(z,X)),s.closePath(),s.stroke();let O=2.4;s.globalAlpha=.6,s.setLineDash([4,4]),s.beginPath(),s.moveTo(...y(-O,-O)),s.lineTo(...y(O,-O)),s.lineTo(...y(O,O)),s.lineTo(...y(-O,O)),s.closePath(),s.stroke(),s.restore()}function ee(C,L){let[O,z]=y(C,L,en);s.save(),s.translate(O,z),s.scale(1,.5);let X=s.createRadialGradient(0,0,0,0,0,36*u);X.addColorStop(0,"rgba(255, 214, 140, 0.5)"),X.addColorStop(1,"rgba(255, 214, 140, 0)"),s.fillStyle=X,s.beginPath(),s.arc(0,0,36*u,0,Math.PI*2),s.fill(),s.restore()}function J(C,L,O,z,X,Z,re,fe){let me=(se,ge,_e)=>y(se,ge,_e),B=z,ue=z+X;k([me(C+O,L-O,ue),me(C+O,L+O,ue),me(C+O,L+O,B),me(C+O,L-O,B)],fe),k([me(C+O,L+O,ue),me(C-O,L+O,ue),me(C-O,L+O,B),me(C+O,L+O,B)],re),k([me(C-O,L-O,ue),me(C+O,L-O,ue),me(C+O,L+O,ue),me(C-O,L+O,ue)],Z)}function ie(C,L,O,z){let X=en;J(C,L,.42,X,9,"hsl(32 30% 74%)","hsl(32 25% 52%)","hsl(32 25% 60%)");let Z=C,re=L-.12,fe=.12;J(Z,re,fe,X+9,11,p.ink2,p.ink,p.ink2);let me=(B,ue)=>y(B,re+fe,ue);s.save(),O&&(s.shadowColor=p.info,s.shadowBlur=8+4*Math.sin(z/400)),k([me(Z-fe*.8,X+11),me(Z+fe*.8,X+11),me(Z+fe*.8,X+18),me(Z-fe*.8,X+18)],O?p.info:p.rule),s.restore()}function ce(C,L){let O=en;J(C,L,.2,O,6,"hsl(18 35% 52%)","hsl(18 30% 38%)","hsl(18 30% 44%)");let[z,X]=y(C,L,O+6),Z=[[-4,-9,-.6],[4,-9,.6],[0,-13,0],[-2,-6,-1.2],[3,-6,1.1]];s.save(),s.fillStyle="hsl(118 38% 40%)";for(let[re,fe,me]of Z)s.beginPath(),s.ellipse(z+re*u,X+fe*u,5*u,2.8*u,me,0,Math.PI*2),s.fill();s.restore()}function we(C,L){J(C,L,.22,en,10,"hsl(35 45% 64%)","hsl(35 40% 42%)","hsl(35 40% 50%)")}function pe(C,L){let[O,z]=y(C,L,en),[X,Z]=y(C,L,en+44);s.save(),s.strokeStyle=p.ink2,s.lineWidth=1.5,s.beginPath(),s.moveTo(O,z),s.lineTo(X,Z),s.stroke(),s.shadowColor="hsl(42 95% 62%)",s.shadowBlur=10,s.fillStyle="hsl(42 95% 74%)",s.beginPath(),s.arc(X,Z,2.8,0,Math.PI*2),s.fill(),s.restore()}function Pe(C,L,O){let z=en;J(C,L,.22,z,26,"hsl(220 10% 34%)","hsl(220 10% 16%)","hsl(220 10% 24%)");let X=L+.22;for(let Z=0;Z<5;Z++)for(let re=0;re<2;re++){let[fe,me]=y(C-.1+re*.12,X,z+4+Z*4.5),B=Math.sin(O/170+Z*1.7+re*2.9)>.1;s.beginPath(),s.arc(fe,me,1.3,0,Math.PI*2),s.fillStyle=B?re?p.ok:p.info:p.ink3,s.fill()}}function Ve(C,L){let O=en;J(C,L,.3,O,24,"hsl(28 30% 62%)","hsl(28 28% 40%)","hsl(28 28% 48%)");let z=L+.3;for(let X=0;X<2;X++){let Z=O+4+X*10;for(let re=0;re<6;re++){let fe=C-.26+re*.09,me=[y(fe,z,Z+7),y(fe+.07,z,Z+7),y(fe+.07,z,Z),y(fe,z,Z)];k(me,`hsl(${(re*53+X*140)%360} 45% ${X?46:58}%)`)}}}function Oe(C,L,O){let z=en;s.save(),s.strokeStyle=p.ink2,s.lineWidth=1.2;for(let Z of[C-.5,C+.5]){let[re,fe]=y(Z,L,z),[me,B]=y(Z,L,z+28);s.beginPath(),s.moveTo(re,fe),s.lineTo(me,B),s.stroke()}let X=[y(C-.55,L,z+26),y(C+.55,L,z+26),y(C+.55,L,z+12),y(C-.55,L,z+12)];k(X,p.sheet),s.strokeStyle=p.ink3,s.lineWidth=1,s.beginPath(),X.forEach(([Z,re],fe)=>fe===0?s.moveTo(Z,re):s.lineTo(Z,re)),s.closePath(),s.stroke(),s.strokeStyle=O?p.info:p.signal,s.beginPath();for(let Z=0;Z<3;Z++){let re=z+22-Z*3.5,[fe,me]=y(C-.4,L,re),[B,ue]=y(C-.4+.25+Z*.12,L,re);s.moveTo(fe,me),s.lineTo(B,ue)}s.stroke(),s.restore()}function oe(C,L){J(C,L,.3,0,6,"hsl(30 22% 66%)","hsl(30 20% 44%)","hsl(30 20% 52%)")}function le(C,L){let O=a.some(z=>z.district===C.d&&z.agent.status==="working");switch(C.kind){case"desk":ie(C.x,C.y,O,L);break;case"plant":ce(C.x,C.y);break;case"crate":we(C.x,C.y);break;case"lamp":pe(C.x,C.y);break;case"rack":Pe(C.x,C.y,L);break;case"shelf":Ve(C.x,C.y);break;case"board":Oe(C.x,C.y,O);break;case"bench":oe(C.x,C.y);break}}function be(C){let L=o.find(re=>re.role==="orchestrator");if(!L)return;let[O,z]=y(L.cx,L.cy,en+60+Math.sin(C/900)*2),X=22*u,Z=C/2600;s.save(),s.globalAlpha=.7,s.strokeStyle=p.signal,s.lineWidth=1.4,s.setLineDash([6*u,5*u]),s.lineDashOffset=-C/60,s.beginPath(),s.ellipse(O,z,X,X*.36,Z,0,Math.PI*2),s.stroke(),s.globalAlpha=.4,s.strokeStyle=p.info,s.setLineDash([]),s.beginPath(),s.ellipse(O,z,X*.66,X*.24,-Z*1.6,0,Math.PI*2),s.stroke(),s.restore()}function Ge(C,L){let[O,z]=y(C.x,C.y,en),X=C.agent.status,Z=X==="working"?p.info:X==="idle"?p.ok:p.ink3,re=C.district.hue,fe=C.moving?Math.sin(C.walk)*3:0,me=ue=>X==="working"?Math.sin(L/110+(ue>0?Math.PI:0))*2:0;s.save(),s.globalAlpha=X==="offline"?.4:1,s.translate(O,z),s.scale(u*C.facing,u),s.save(),s.globalAlpha*=.18,s.fillStyle=p.ink,s.beginPath(),s.ellipse(0,0,9,4.5,0,0,Math.PI*2),s.fill(),s.restore(),s.strokeStyle=p.ink2,s.lineWidth=2.5,s.lineCap="round";for(let ue of[-1,1]){let se=ue===-1?fe:-fe;s.beginPath(),s.moveTo(ue*3.5,-12),s.lineTo(ue*3.5+se,0),s.stroke()}let B=s.createLinearGradient(0,-30,0,-12);B.addColorStop(0,`hsl(${re} 55% 66%)`),B.addColorStop(1,`hsl(${re} 50% 44%)`),D(-7,-30,14,18,4),s.fillStyle=B,s.fill(),s.strokeStyle=p.ink2,s.lineWidth=.8,s.stroke(),D(-4,-25,8,7,1.5),s.fillStyle=p.sheet,s.fill(),s.fillStyle=p.ink,s.font="bold 7px ui-monospace, SFMono-Regular, Menlo, monospace",s.textAlign="center",s.textBaseline="middle",s.fillText(C.agent.role.charAt(0).toUpperCase()||"?",0,-21.5),s.strokeStyle=`hsl(${re} 45% 36%)`,s.lineWidth=2;for(let ue of[-1,1])s.beginPath(),s.moveTo(ue*7,-27),s.lineTo(ue*10,-19+me(ue)),s.stroke();D(-6,-41,12,10,3),s.fillStyle=p.sheet,s.fill(),s.strokeStyle=p.ink2,s.stroke(),D(-4.5,-38.5,9,5,1.5),s.fillStyle=p.ink,s.fill(),s.fillStyle=Z;for(let ue of[-2,2])s.beginPath(),s.arc(ue,-36,1.1,0,Math.PI*2),s.fill();if(s.strokeStyle=p.ink2,s.lineWidth=.8,s.beginPath(),s.moveTo(0,-41),s.lineTo(0,-46),s.stroke(),s.beginPath(),s.arc(0,-47,1.8,0,Math.PI*2),s.fillStyle=Z,s.fill(),X==="working"){let ue=.5+.5*Math.sin(L/300);s.shadowColor=p.info,s.shadowBlur=8+6*ue,s.beginPath(),s.arc(0,-56,2.6+ue*.6,0,Math.PI*2),s.fillStyle=p.info,s.fill(),s.shadowBlur=0}s.restore()}function Me(C,L){let[O,z]=y(C.x,C.y,en);if(C.bubble&&L<C.bubble.until){let[X,Z]=y(C.x,C.y,en+52),re=Sr((C.bubble.until-L)/800,0,1),fe=jt(C.bubble.text,30);s.font="11px ui-sans-serif, system-ui, sans-serif";let me=s.measureText(fe).width+14,B=18;s.save(),s.globalAlpha=re,D(X-me/2,Z-B-6,me,B,6),s.fillStyle=p.sheet,s.fill(),s.strokeStyle=C.bubble.tone==="danger"?p.danger:C.bubble.tone==="ok"?p.ok:p.info,s.lineWidth=1,s.stroke(),s.fillStyle=p.ink,s.textAlign="center",s.textBaseline="middle",s.fillText(fe,X,Z-B/2-6),s.restore()}(C===f||C===h||u>.9)&&(s.font="600 11px ui-sans-serif, system-ui, sans-serif",s.textAlign="center",s.textBaseline="top",s.lineJoin="round",s.lineWidth=3,s.strokeStyle=p.paper,s.strokeText(C.agent.name,O,z+14),s.fillStyle=p.ink,s.fillText(C.agent.name,O,z+14))}function ze(C){for(let L=d.length-1;L>=0;L--){let O=d[L];if(!O)continue;let z=(C-O.start)/B_;if(z>=1){d.splice(L,1);continue}let X=c.get(O.from),Z=c.get(O.to);if(!X||!Z)continue;let[re,fe]=y(X.x,X.y,en+gf),[me,B]=y(Z.x,Z.y,en+gf),ue=(re+me)/2,se=(fe+B)/2-60*u,ge=1-z,_e=ge*ge*re+2*ge*z*ue+z*z*me,he=ge*ge*fe+2*ge*z*se+z*z*B,Ce=O.tone==="ok"?p.ok:p.signal;s.save(),s.shadowColor=Ce,s.shadowBlur=12,s.fillStyle=Ce,s.beginPath(),s.arc(_e,he,3.5,0,Math.PI*2),s.fill(),s.restore()}}function Ut(C){s.setTransform(v,0,0,v,0,0),s.globalAlpha=1,s.fillStyle=p.paper,s.fillRect(0,0,g,_);let L=8;for(let z of o)L=Math.max(L,Math.abs(z.cx)+z.r+3,Math.abs(z.cy)+z.r+3);s.save(),s.globalAlpha=.3,s.strokeStyle=p.rule,s.lineWidth=1,s.beginPath();for(let z=-L;z<=L;z++){let[X,Z]=y(z,-L),[re,fe]=y(z,L);s.moveTo(X,Z),s.lineTo(re,fe);let[me,B]=y(-L,z),[ue,se]=y(L,z);s.moveTo(me,B),s.lineTo(ue,se)}s.stroke(),s.restore(),$();for(let z of o)N(z);for(let z of l)z.kind==="lamp"&&ee(z.x,z.y);let O=[];for(let z of l)O.push({depth:z.x+z.y,paint:()=>le(z,C)});for(let z of a){let X=z.agent.status==="working",[Z,re]=z.home;O.push({depth:Z+re-.01,paint:()=>H(Z,re,X)}),O.push({depth:z.x+z.y,paint:()=>Ge(z,C)})}O.sort((z,X)=>z.depth-X.depth);for(let z of O)z.paint();be(C),ze(C);for(let z of a)Me(z,C);tn()}function Je(C,L,O){C!==L&&(d.push({from:C,to:L,start:performance.now(),tone:O}),d.length>H_&&d.shift())}function st(C,L,O){C.bubble={text:L,until:performance.now()+O_,tone:O}}function gt(C,L){let O=null,z=22*u;for(let X of a){let[Z,re]=y(X.x,X.y,en),fe=Math.hypot(C-Z,L-(re-24*u));fe<z&&(z=fe,O=X)}return O}function tt(C){h=C,M="",W()}let Tt=C=>C.agent.status==="offline"?"offline":C.activity,Ft=C=>{let L=typeof C=="number"?c.get(C):void 0;return L?L.agent.name:String(C??"?")};function Wt(C,L){C.activity=L,C===h&&W()}function Pt(C,L){let O=L.to_cid??L.assignee;return(C==="plaza_send_message"||(C==="plaza_assign"||C==="plaza_post_task"))&&typeof O=="number"?O:null}function Ot(C,L){if(C==="bash")return`$ ${jt(Pi(L.command).split(`
`)[0]??"",120)}`;if(C==="plaza_send_message")return`\u2709 to ${Ft(L.to_cid)}: ${jt(Pi(L.text),100)}`;if(C==="plaza_assign"||C==="plaza_post_task"&&L.assignee!=null)return`\u2192 task for ${Ft(L.assignee??L.to_cid)}: ${Pi(L.title)}`;if(C==="share_file")return`\u21EA shared ${Pi(L.path)}`;let O=Pi(L.path)||Pi(L.url);return O?`\u2192 ${C} ${jt(O,80)}`:`\u2192 ${C}`}function W(){if(!h){t.hidden=!0,M="";return}let C=h.agent;t.hidden=!1;let L=Tt(h),O=[C.name,L].join("|");O!==M&&(M=O,t.replaceChildren(x("header.floor-card-head",null,x("strong",null,C.name)),x("div.floor-card-activity",null,L)))}function tn(){if(!h||t.hidden)return;let[C,L]=y(h.x,h.y,en+40),O=Sr(C+24,8,Math.max(8,g-308)),z=Sr(L-120,8,Math.max(8,_-260));t.style.left=`${O}px`,t.style.top=`${z}px`}let ct=C=>{let L=e.getBoundingClientRect();return[C.clientX-L.left,C.clientY-L.top]};e.addEventListener("pointerdown",C=>{P={x:C.clientX,y:C.clientY,moved:0},e.setPointerCapture(C.pointerId),e.style.cursor="grabbing"}),e.addEventListener("pointermove",C=>{if(P){let z=C.clientX-P.x,X=C.clientY-P.y;P.moved+=Math.abs(z)+Math.abs(X),P.x=C.clientX,P.y=C.clientY,m={x:m.x+z,y:m.y+X};return}let[L,O]=ct(C);f=gt(L,O),e.style.cursor=f?"pointer":"grab"}),e.addEventListener("pointerup",C=>{if(!P)return;let L=P.moved;if(P=null,e.style.cursor="grab",L<4){let[O,z]=ct(C);tt(gt(O,z))}}),e.addEventListener("pointercancel",()=>{P=null,e.style.cursor="grab"}),e.addEventListener("pointerleave",()=>{f=null}),e.addEventListener("wheel",C=>{C.preventDefault();let[L,O]=ct(C),z=g/2+m.x,X=_/2+m.y,Z=(L-z)/u,re=(O-X)/u;u=Sr(u*Math.exp(-C.deltaY*.0015),.4,2.5),m={x:L-Z*u-g/2,y:O-re*u-_/2}},{passive:!1});let F=()=>{v=window.devicePixelRatio||1,g=e.clientWidth,_=e.clientHeight,e.width=Math.round(g*v),e.height=Math.round(_*v)},b=new ResizeObserver(()=>F());b.observe(e),F();let Y=ft.watch(C=>{S(C),!w&&C.length&&(R(),w=!0)}),te=Mt("transcript",C=>{let L=C,O=c.get(L.cid);switch(L.kind){case"tool_request":{let z=Pi(L.data.tool)||Pi(L.data.name)||"tool",X=L.data.input??{};O&&(st(O,`\u2192 ${z}`,"info"),Wt(O,Ot(z,X)));let Z=Pt(z,X);Z!==null&&Je(L.cid,Z,"accent");break}case"model_request":O&&Wt(O,"thinking\u2026");break;case"work_end":{let z=Pi(L.data.status)||"done";O&&(st(O,z,/err|fail/i.test(z)?"danger":"ok"),Wt(O,"idle"));break}case"work_start":{let z=L.data.origin??{};if(typeof z.from_cid=="number")Je(z.from_cid,L.cid,"accent");else if(L.data.kind==="task"){let X=a.find(Z=>Z.agent.tier==="orchestrator");X&&Je(X.agent.cid,L.cid,"accent")}break}default:break}}),ae=Mt("delta",C=>{let L=c.get(C.cid);L&&Wt(L,"thinking\u2026")});e.addEventListener("dblclick",C=>{let[L,O]=ct(C),z=gt(L,O);z&&Mn("agents",z.agent.cid)});let ve=Mt("conversation",C=>{let L=C;Je(L.from_cid,L.to_cid,"ok")}),ye=C=>{if(!T)return;let L=Math.min(.05,(C-U)/1e3);U=C,I(L,C),Ut(C),E=requestAnimationFrame(ye)};return E=requestAnimationFrame(ye),()=>{T=!1,cancelAnimationFrame(E),b.disconnect(),Y(),te(),ae(),ve(),n.classList.remove("floor-view"),n.replaceChildren()}}var j_=1500,Q_=1.6,ey=60,Mo=.5,Il=.8,yo=2.3,wo=1.5,Su=2.2,ty=9.8,ny=.95,iy=Math.PI/4,ry=9.5,yf=3.1,So=.55,sy=[0,30,-30,60,-60].map(n=>n*Math.PI/180),wu=90,oy="position:absolute;left:0;top:0;display:none;max-width:220px;padding:3px 6px;background:var(--sheet);border:1px solid var(--ink);color:var(--ink);font:11px/1.35 ui-monospace,SFMono-Regular,Menlo,monospace;white-space:nowrap;pointer-events:none;will-change:transform",bf=[8,28,46,88,140,172,198,222,262,340];function ay(n){return(n*137.508+20)%360}function ly(n){let e=0;for(let t of n)e=e*31+t.charCodeAt(0)>>>0;return bf[e%bf.length]}function cy(n){let e=new Map;for(let d of n){let m=d.tier==="orchestrator"?"":d.role;e.set(m,[...e.get(m)??[],d])}let t=[],i=new Map,r=(d,m)=>{t.push(d);let u=Math.max(1,Math.ceil(Math.sqrt(m.length)));m.sort((g,_)=>g.cid-_.cid).forEach((g,_)=>{let v=1.5*(u-1),f=d.cx-v+_%u*3,h=d.cy-v+Math.floor(_/u)*3;i.set(g.cid,[d,f,h])})},s=e.get("");s&&r({role:"orchestrator",cx:0,cy:0,r:2,hue:25},s);let o=[...e.keys()].filter(d=>d).sort(),a=d=>Math.ceil(1.5*Math.max(1,Math.ceil(Math.sqrt(d)))+1),l=o.reduce((d,m)=>Math.max(d,a(e.get(m).length)),0),c=Math.max(9+wo,Math.ceil((2*(l+wo)+1.5)/(2*Math.sin(Math.PI/Math.max(3,o.length)))));return o.forEach((d,m)=>{let u=e.get(d),g=m/o.length*Math.PI*2-Math.PI/2;r({role:d,cx:Math.round(Math.cos(g)*c),cy:Math.round(Math.sin(g)*c),r:a(u.length),hue:ly(d)},u)}),{districts:t,homes:i}}function uy(){let n=getComputedStyle(document.documentElement),e=(t,i)=>n.getPropertyValue(`--${t}`).trim()||i;return{paper:e("paper","#f5f3ee"),sheet:e("sheet","#ffffff"),rule:e("rule","#d6d2c8"),ink:e("ink","#1a1a1a"),ink2:e("ink-2","#4a4a4a"),ink3:e("ink-3","#8a8a8a"),signal:e("signal","#d9482b"),ok:e("ok","#2f8f5b"),info:e("info","#2f6fb0"),danger:e("danger","#c0392b")}}var wr=null;function Pl(n,e){wr??=document.createElement("canvas").getContext("2d");let t=new Ne;if(!wr)return t.set(e);wr.fillStyle=e,wr.fillStyle=n,wr.clearRect(0,0,1,1),wr.fillRect(0,0,1,1);let i=wr.getImageData(0,0,1,1).data;return t.setRGB((i[0]??0)/255,(i[1]??0)/255,(i[2]??0)/255,kt)}function nr(n){return typeof n=="string"?n:""}function Eu(n,e,t){return Math.min(t,Math.max(e,n))}var bo=n=>n.role==="orchestrator"?Il:Mo,Mf=(n,e,t)=>e>n.x0-So&&e<n.x1+So&&t>n.z0-So&&t<n.z1+So;function Er(n){n.traverse(e=>{let t=e;t.geometry?.dispose();let i=Array.isArray(t.material)?t.material:t.material?[t.material]:[];for(let r of i)r.map?.dispose(),r.dispose()})}function hy(n,e){let t=e.to_cid??e.assignee;return(n==="plaza_send_message"||(n==="plaza_assign"||n==="plaza_post_task"))&&typeof t=="number"?t:null}function dy(n,e,t){if(n==="bash")return`$ ${jt(nr(e.command).split(`
`)[0]??"",120)}`;if(n==="plaza_send_message")return`\u2709 to ${t(e.to_cid)}: ${jt(nr(e.text),100)}`;if(n==="plaza_assign"||n==="plaza_post_task"&&e.assignee!=null)return`\u2192 task for ${t(e.assignee??e.to_cid)}: ${nr(e.title)}`;if(n==="share_file")return`\u21EA shared ${nr(e.path)}`;let i=nr(e.path)||nr(e.url);return i?`\u2192 ${n} ${jt(i,80)}`:`\u2192 ${n}`}function fy(){try{return new Ml({antialias:!0})}catch{return null}}function Sf(n){let e=document.createElement("canvas"),i=e.getContext("webgl2")??e.getContext("webgl")?fy():null;if(!i)return n.replaceChildren(x("p.faint",null,"3D unavailable (no WebGL) \u2014 showing the 2D floor.")),_f(n);let r=uy(),s={sheet:new Ne(16774366),rule:new Ne(14858623),ink3:new Ne(6715538),signal:Pl(r.signal,"#d9482b"),ok:Pl(r.ok,"#2f8f5b"),info:Pl(r.info,"#2f6fb0"),paper:Pl(r.paper,"#f5f3ee")},o={ink:r.ink,sheet:r.sheet,paper:r.paper,rule:r.rule,signal:r.signal,ok:r.ok,info:r.info,inkMuted:r.ink3};i.setPixelRatio(Math.min(window.devicePixelRatio||1,2)),i.shadowMap.enabled=!0,i.shadowMap.type=mr;let a=i.domElement;a.classList.add("floor-canvas"),a.setAttribute("aria-label","The plaza floor"),a.style.cssText="position:absolute;inset:0;width:100%;height:100%;display:block;cursor:grab";let l=new Ns,c=new Ne(6269160),d=new Ne(16640452);l.add(Kd(c,d,new Ne(9424226))),l.fog=new Ds(d,70,240);let m=Jd(14,70,ps(31));l.add(m.group);let u=new gn(40,1,.5,1e3),g=new Tl(u,a);g.enableDamping=!0,g.dampingFactor=.08,g.enablePan=!0,g.enableZoom=!0,g.minPolarAngle=.2,g.maxPolarAngle=1.3,g.minDistance=6,g.maxDistance=300,l.add(new Zs(14676223,9075290,1.1));let _=new Qs(16773590,2.6);_.position.set(22,40,16),_.castShadow=!0,_.shadow.mapSize.set(2048,2048),_.shadow.bias=-8e-4,_.shadow.normalBias=.03;let v=_.shadow.camera;Object.assign(v,{left:-40,right:40,top:40,bottom:-40,near:1,far:140}),v.updateProjectionMatrix(),l.add(_);let f=new rt(new Vi(500,500),Et({map:vu()}));vu().repeat.set(36,36),f.rotation.x=-Math.PI/2,f.position.y=-.01,f.receiveShadow=!0,l.add(f);let h=new ht;l.add(h);let M=new ht;l.add(M);let P=df(s);l.add(P);let w=P.userData.animate,T=new di(.42,28),E=new Vt(.13,14,10),U=new Vt(.34,14,10),p=new Qe(.2,.2,.2),S={accent:new ln({color:s.signal,toneMapped:!1}),ok:new ln({color:s.ok,toneMapped:!1}),accentHalo:new ln({color:s.signal,transparent:!0,opacity:.35,depthWrite:!1,blending:os,toneMapped:!1}),okHalo:new ln({color:s.ok,transparent:!0,opacity:.35,depthWrite:!1,blending:os,toneMapped:!1}),token:new ln({color:s.signal,toneMapped:!1})},y=new Float32Array(wu*3),R=new Pn(y,3),I=[];for(let q=0;q<wu;q++){let de=Math.random()*Math.PI*2,xe=Math.sqrt(Math.random())*9;I.push({x:Math.cos(de)*xe,y:Math.random()*4.2,z:Math.sin(de)*xe,rise:.04+Math.random()*.05})}let k=new $t;k.setAttribute("position",R);let D=new Qr({color:s.ink3,size:.07,transparent:!0,opacity:.55,depthWrite:!1});l.add(new zs(k,D));let N=x("div.floor-card",{hidden:!0}),H=x("div.floor-legend",null,x("span",null,x("i.floor-key.working"),"Working"),x("span",null,x("i.floor-key.idle"),"Idle"),x("span",null,x("i.floor-key.offline"),"Offline")),$=document.createElement("div");$.style.cssText="position:absolute;inset:0;overflow:hidden;pointer-events:none";let ee=x("div.floor-stage",null,$,N,H);ee.style.position="relative",ee.prepend(a),n.classList.add("floor-view"),document.body.classList.add("floor-mode"),n.append(x("div.view-head",null,x("h1",null,"Floor"),x("div.actions",null,Ye("Reset view",()=>Y(),{kind:"ghost",icon:"refresh"}))),ee);let J=[],ie="",ce=[],we=new Map,pe=[],Pe=[],Ve=new Map,Oe=[],oe=new G,le=0,be=0,Ge=null,Me=null,ze="",Ut=!1,Je=performance.now(),st=new We,gt=new eo,tt=new G,Tt=new G(0,1,0),Ft=new G,Wt=new an,Pt=new an;function Ot(q,de){let xe=document.createElement("canvas");xe.width=512,xe.height=128;let K=xe.getContext("2d");K.beginPath(),K.moveTo(24,4),K.arcTo(508,4,508,124,20),K.arcTo(508,124,4,124,20),K.arcTo(4,124,4,4,20),K.arcTo(4,4,508,4,20),K.closePath(),K.fillStyle=r.sheet,K.fill(),K.lineWidth=6,K.strokeStyle=de?r.signal:r.rule,K.stroke(),K.fillStyle=r.ink2,K.textAlign="center",K.textBaseline="middle";let Ie="500 48px ui-monospace, SFMono-Regular, Menlo, monospace";K.font=Ie;let Se=K.measureText(q).width;Se>460&&(K.font=`500 ${Math.floor(48*460/Se)}px ui-monospace, SFMono-Regular, Menlo, monospace`),K.fillText(q,256,66);let Ke=new pr(xe);return Ke.colorSpace=kt,Ke}function W(q){let de=new ht,xe=q.role==="orchestrator";if(xe){let Ie=new rt(new zt(yo,yo+Su,Il,48),Et({map:vo(new Ne(15782818),3)}));Ie.position.set(q.cx,Il/2,q.cy),Ie.castShadow=!0,Ie.receiveShadow=!0;let Se=new rt(new Wi(yo,.05,8,64),new ln({color:s.signal}));Se.rotation.x=Math.PI/2,Se.position.set(q.cx,Il+.01,q.cy),de.add(Ie,Se)}else{let Ie=new zt(q.r*Math.SQRT2,(q.r+wo)*Math.SQRT2,Mo,4,1);Ie.rotateY(Math.PI/4);let Se=new rt(Ie,[yu(q.hue),Et({map:rf(q.hue)}),yu(q.hue)]);Se.position.set(q.cx,Mo/2,q.cy),Se.castShadow=!0,Se.receiveShadow=!0,de.add(Se);let Ke=hf(q.hue,q.r,q.role,s);Ke.position.set(q.cx,Mo,q.cy),de.add(Ke),Ke.updateMatrixWorld(!0);for(let Ht of Ke.children){let dt=new Dn().setFromObject(Ht);dt.min.y>Mo+2.2||Pe.push({x0:dt.min.x,x1:dt.max.x,z0:dt.min.z,z1:dt.max.z})}Ve.set(q.role,{screens:Ke.userData.screens??[],lit:!1});let Xe=Ke.userData.animate;Xe&&Oe.push(Xe)}let K=new rt(new Vi(2.6,.65),new ln({map:Ot(q.role.toUpperCase(),xe),transparent:!0,depthWrite:!1}));return K.rotation.x=-Math.PI/2,K.position.set(q.cx,bo(q)+.01,xe?q.cy-1.6:q.cy-q.r+.55),de.add(K),Ci(de),de}function tn(q){for(let Se of[...h.children])h.remove(Se),Er(Se);Ve.clear(),Oe.length=0,Pe.length=0;for(let Se of q)h.add(W(Se));Er(M),M.clear(),M.add(pf(q.map(Se=>new G(Se.cx,0,Se.cy)),s));let de=ff(s,q.map(Se=>Math.atan2(Se.cy,Se.cx)));M.add(de.group),Oe.push(de.animate);for(let Se of de.boxes)Pe.push(Se);let xe=mf(q,s);M.add(xe.group);for(let Se of xe.boxes)Pe.push(Se);let K=12;for(let Se of q)K=Math.max(K,Math.hypot(Se.cx,Se.cy)+Se.r+10);let Ie=jd(9e3,Math.min(K,60),(Se,Ke)=>{if(Math.hypot(Se,Ke)<4.8)return!1;for(let Xe of q){let Ht=Xe.role==="orchestrator"?yo+Su:Xe.r+wo;if(Math.abs(Se-Xe.cx)<Ht+.2&&Math.abs(Ke-Xe.cy)<Ht+.2)return!1;let dt=Math.hypot(Xe.cx,Xe.cy);if(dt>.5){let _n=Eu((Se*Xe.cx+Ke*Xe.cy)/(dt*dt),0,1);if(Math.hypot(Se-Xe.cx*_n,Ke-Xe.cy*_n)<.75)return!1}}return!Pe.some(Xe=>Se>Xe.x0-.1&&Se<Xe.x1+.1&&Ke>Xe.z0-.1&&Ke<Xe.z1+.1)},ps(97));M.add(Ie.mesh),Oe.push(Ie.animate)}function ct(q,de,xe,K){let Ie=q.tier==="orchestrator",Se=ef(de.hue,Ie,o,ay(q.cid));Se.group.traverse(ir=>{ir.castShadow=!0,ir.receiveShadow=!0}),l.add(Se.group);let Ke=new ln({color:s.ink3,transparent:!0,opacity:.6,depthWrite:!1}),Xe=new rt(T,Ke);Xe.rotation.x=-Math.PI/2,Xe.position.set(xe,bo(de)+.02,K),l.add(Xe);let Ht=new rt(p,S.token);Ht.visible=!1,l.add(Ht);let dt=document.createElement("div");dt.style.cssText=oy;let _n=document.createElement("strong");_n.style.cssText="display:block;overflow:hidden;text-overflow:ellipsis";let vi=document.createElement("div");return vi.style.cssText="color:var(--ink-2);overflow:hidden;text-overflow:ellipsis",dt.append(_n,vi),$.append(dt),{agent:q,district:de,home:[xe,K],x:xe,y:K,tx:xe,ty:K,heading:0,moving:!1,py:0,vy:0,state:null,nextWander:performance.now()+Math.random()*3e3,nextSocial:performance.now()+4e3+Math.random()*1e4,social:null,activity:"idle",robot:Se,robotKey:`${de.hue}|${Ie}`,tilt:new an,pad:Xe,padMat:Ke,token:Ht,highlighted:!1,label:dt,labelName:_n,labelLine:vi}}function F(q){l.remove(q.robot.group),q.robot.dispose(),l.remove(q.pad),q.padMat.dispose(),l.remove(q.token),q.label.remove()}function b(q){let de=cy(q);J=de.districts;let xe=JSON.stringify(J);xe!==ie&&(ie=xe,tn(J));let K=new Set;for(let Ie of q){let Se=de.homes.get(Ie.cid);if(!Se)continue;let[Ke,Xe,Ht]=Se;K.add(Ie.cid);let dt=we.get(Ie.cid);dt&&dt.robotKey!==`${Ke.hue}|${Ie.tier==="orchestrator"}`&&(F(dt),dt=void 0),dt||(dt=ct(Ie,Ke,Xe,Ht),we.set(Ie.cid,dt)),Object.assign(dt,{agent:Ie,district:Ke,home:[Xe,Ht]}),dt.pad.position.set(Xe,bo(Ke)+.02,Ht)}for(let[Ie,Se]of we)K.has(Ie)||(F(Se),we.delete(Ie));ce=[...we.values()],Me&&(Me=we.get(Me.agent.cid)??null),Ge&&(Ge=we.get(Ge.agent.cid)??null),se()}function Y(){let q=6;for(let xe of J)q=Math.max(q,Math.abs(xe.cx)+xe.r,Math.abs(xe.cy)+xe.r);let de=q*2.6+6;g.target.set(0,.4,0),u.position.copy(g.target).add(new G().setFromSphericalCoords(de,ny,iy)),g.update()}let te=(q,de)=>q.role===de.role||Math.hypot(q.cx-de.cx,q.cy-de.cy)<=ry;function ae(q,de){if(q.nextSocial=de+8e3+Math.random()*12e3,Math.random()<.3){let Ie=Math.atan2(q.y,q.x);q.social={phase:"stroll",partner:null,gx:Math.cos(Ie)*yf,gy:Math.sin(Ie)*yf,until:1/0};return}let xe=ce.filter(Ie=>Ie!==q&&Ie.agent.status==="idle"&&!Ie.social&&te(Ie.district,q.district)),K=xe[Math.floor(Math.random()*xe.length)];K&&(q.social={phase:"go",partner:K,gx:K.x-1,gy:K.y,until:1/0},K.social={phase:"talk",partner:q,gx:K.x,gy:K.y,until:1/0})}function ve(q,de){let xe=q.social;q.social=null,q.nextWander=de,xe?.partner&&xe.partner.social?.partner===q&&(xe.partner.social=null,xe.partner.nextWander=de)}function ye(q,de){let xe=0;for(let K of J){let Ie=K.role==="orchestrator",Se=Ie?Math.hypot(q-K.cx,de-K.cy)-yo:Math.max(Math.abs(q-K.cx),Math.abs(de-K.cy))-K.r,Ke=Ie?Su:wo,Xe=Se<=0?bo(K):Se<Ke?bo(K)*(1-Se/Ke):0;xe=Math.max(xe,Xe)}return xe}function C(q,de,xe,K){if(K){for(let Se of Pe)if(Mf(Se,de,xe)&&!Mf(Se,q.x,q.y))return!0}let Ie=q.social?.partner??null;for(let Se of ce){if(Se===q||Se===Ie)continue;let Ke=Math.hypot(Se.x-de,Se.y-xe);if(Ke<So*2&&Ke<Math.hypot(Se.x-q.x,Se.y-q.y))return!0}return!1}function L(q,de){let xe=de/1e3;for(let K of Ve.values())K.lit=!1;for(let K of ce){let Ie=K.agent.status,Se=Ie==="idle",Ke=K.social?.partner??null;if(K.social&&(!Se||Ke&&(we.get(Ke.agent.cid)!==Ke||Ke.agent.status!=="idle"))&&ve(K,de),Se&&!K.social){if(de>=K.nextWander){let Ae=Math.max(.5,K.district.r-.7);K.tx=K.district.cx+(Math.random()*2-1)*Ae,K.ty=K.district.cy+(Math.random()*2-1)*Ae,K.nextWander=de+3e3+Math.random()*4e3}de>=K.nextSocial&&ae(K,de)}let Xe=K.social,Ht=K.home[0],dt=K.home[1];Se&&(Ht=Xe?Xe.gx:K.tx,dt=Xe?Xe.gy:K.ty);let _n=Ht-K.x,vi=dt-K.y,ir=Math.hypot(_n,vi);K.moving=ir>.02;let zl=Math.hypot(K.home[0]-K.x,K.home[1]-K.y)>.9,To=null;if(K.moving){let Ae=Math.min(Q_*q,ir),Ue=Math.atan2(_n,vi);for(let et of sy){let nt=Ue+et,Fe=K.x+Math.sin(nt)*Ae,xt=K.y+Math.cos(nt)*Ae;if(!C(K,Fe,xt,zl)){K.x=Fe,K.y=xt,To=nt;break}}}if(Xe&&Se){if(Xe.phase==="go"&&!K.moving){let Ae=de+3e3+Math.random()*2e3;Xe.phase="talk",Xe.until=Ae,Xe.partner?.social&&(Xe.partner.social.until=Ae)}else Xe.phase==="stroll"&&!K.moving&&Xe.until===1/0&&(Xe.until=de+2e3);de>=Xe.until&&ve(K,de)}let Tr=Xe&&Xe.phase==="talk"?Xe.partner:null,A=null;if(Tr?A=Math.atan2(Tr.x-K.x,Tr.y-K.y):K.moving&&(A=To??Math.atan2(_n,vi)),A!==null){let Ae=Math.atan2(Math.sin(A-K.heading),Math.cos(A-K.heading));K.heading+=Ae*Math.min(1,q*8)}let V=Ie==="working"?"working":Ie==="offline"?"offline":K.moving?"walking":"idle";V!==K.state&&(K.state=V,K.robot.setState(V));let ne=K===Me||K===Ge;ne!==K.highlighted&&(K.highlighted=ne,K.robot.setHighlighted(ne));let Q=ye(K.x,K.y);K.py>Q+.001?(K.vy-=ty*q,K.py=Math.max(Q,K.py+K.vy*q),K.py===Q&&(K.vy=0)):(K.py=Q,K.vy=0),K.robot.group.position.set(K.x,K.py,K.y);let j=.3,Re=(ye(K.x+j,K.y)-ye(K.x-j,K.y))/(2*j),ke=(ye(K.x,K.y+j)-ye(K.x,K.y-j))/(2*j);Ft.set(-Re,1,-ke).normalize(),Wt.setFromUnitVectors(Tt,Ft).multiply(Pt.setFromAxisAngle(Tt,K.heading)),K.tilt.slerp(Wt,1-Math.exp(-10*q)),K.robot.group.quaternion.copy(K.tilt),K.robot.update(q,xe),K.padMat.color.copy(Ie==="working"?s.info:Ie==="idle"?s.ok:s.ink3);let Ee=Ie==="working";if(K.token.visible=Ee,Ee){let Ae=Ve.get(K.district.role);Ae&&(Ae.lit=!0);let Ue=K.robot.labelAnchor(oe);K.token.position.set(Ue.x,Ue.y+.6+Math.sin(xe*2.5+K.agent.cid)*.08,Ue.z),K.token.rotation.y+=q*1.8}}for(let K of Ve.values()){let Ie=K.lit?1.2:.05;for(let Se of K.screens)Se.emissiveIntensity!==Ie&&(Se.emissiveIntensity=Ie)}}function O(q){for(let de=0;de<wu;de++){let xe=I[de],K=de*3;y[K]=xe.x+Math.sin(q*.3+de)*.25,y[K+1]=.3+(xe.y+q*xe.rise)%4.2,y[K+2]=xe.z+Math.cos(q*.25+de)*.25}R.needsUpdate=!0}function z(q,de,xe){if(q===de)return;let K=new rt(E,xe==="ok"?S.ok:S.accent),Ie=new rt(U,xe==="ok"?S.okHalo:S.accentHalo);for(l.add(K,Ie),pe.push({from:q,to:de,start:performance.now(),core:K,halo:Ie});pe.length>ey;)X(pe.shift())}function X(q){l.remove(q.core,q.halo)}function Z(q){for(let de=pe.length-1;de>=0;de--){let xe=pe[de],K=(q-xe.start)/j_,Ie=we.get(xe.from),Se=we.get(xe.to);if(K>=1||!Ie||!Se){X(xe),pe.splice(de,1);continue}let Ke=Ie.robot.labelAnchor(new G),Xe=Se.robot.labelAnchor(new G),Ht=Ke.clone().lerp(Xe,.5);Ht.y+=.8+Ke.distanceTo(Xe)*.3;let dt=1-K,_n=Ke.multiplyScalar(dt*dt).addScaledVector(Ht,2*dt*K).addScaledVector(Xe,K*K);xe.core.position.copy(_n),xe.halo.position.copy(_n)}}let re=q=>q.agent.status==="offline"?"offline":q.social?.phase==="talk"?"\xB7 \xB7 \xB7":q.activity,fe=q=>{let de=typeof q=="number"?we.get(q):void 0;return de?de.agent.name:String(q??"?")};function me(q,de){q.activity=de,q===Me&&se()}function B(q){let de=q.clone().project(u);u.getWorldDirection(tt);let xe=q.clone().sub(u.position).dot(tt)>0;return{x:(de.x+1)/2*le,y:(1-de.y)/2*be,visible:xe&&de.z<=1}}function ue(){for(let q of ce){let de=B(q.robot.labelAnchor(new G));if(!de.visible){q.label.style.display="none";continue}q.label.style.display="block",q.label.style.transform=`translate(${de.x.toFixed(1)}px, ${de.y.toFixed(1)}px) translate(-50%, -100%) translateY(-6px)`,q.labelName.textContent!==q.agent.name&&(q.labelName.textContent=q.agent.name);let xe=re(q);q.labelLine.textContent!==xe&&(q.labelLine.textContent=xe)}}function se(){if(!Me){N.hidden=!0,ze="";return}let q=Me.agent;N.hidden=!1;let de=re(Me),xe=[q.name,de].join("|");xe!==ze&&(ze=xe,N.replaceChildren(x("header.floor-card-head",null,x("strong",null,q.name)),x("div.floor-card-activity",null,de)))}function ge(){if(!Me||N.hidden)return;let q=B(Me.robot.labelAnchor(new G)),de=Eu(q.x+24,8,Math.max(8,le-308)),xe=Eu(q.y-120,8,Math.max(8,be-260));N.style.left=`${de}px`,N.style.top=`${xe}px`}function _e(q){Me=q,ze="",se()}function he(q){let de=a.getBoundingClientRect();st.set((q.clientX-de.left)/de.width*2-1,-((q.clientY-de.top)/de.height)*2+1),gt.setFromCamera(st,u);let xe=gt.intersectObjects(ce.map(K=>K.robot.group),!0);for(let K of xe){let Ie=K.object;for(;Ie;){let Se=ce.find(Ke=>Ke.robot.group===Ie);if(Se)return Se;Ie=Ie.parent}}return null}function Ce(q,de){return dy(q,de,fe)}let Le=()=>{let q=ee.getBoundingClientRect();le=q.width,be=q.height,!(!le||!be)&&(i.setSize(le,be,!1),u.aspect=le/be,u.updateProjectionMatrix())},_t=new ResizeObserver(()=>Le());_t.observe(ee),Le(),Y();let ot=new AbortController,Rt=(q,de)=>a.addEventListener(q,de,{signal:ot.signal}),on=null;Rt("pointerdown",q=>{on={x:q.clientX,y:q.clientY}}),Rt("pointermove",q=>{q.buttons===0&&(Ge=he(q),a.style.cursor=Ge?"pointer":"grab")}),Rt("pointerup",q=>{if(!on)return;let de=Math.hypot(q.clientX-on.x,q.clientY-on.y);on=null,de<4&&_e(he(q))}),Rt("pointerleave",()=>{Ge=null}),Rt("dblclick",q=>{let de=he(q);de&&Mn("agents",de.agent.cid)});let Fl=ft.watch(q=>{b(q),!Ut&&q.length&&(Y(),Ut=!0)}),Ol=Mt("transcript",q=>{let de=q,xe=we.get(de.cid);switch(de.kind){case"tool_request":{let K=nr(de.data.tool)||nr(de.data.name)||"tool",Ie=de.data.input??{};xe&&me(xe,Ce(K,Ie));let Se=hy(K,Ie);Se!==null&&z(de.cid,Se,"accent");break}case"model_request":xe&&me(xe,"thinking\u2026");break;case"work_end":xe&&me(xe,"idle");break;case"work_start":{let K=de.data.origin??{};if(typeof K.from_cid=="number")z(K.from_cid,de.cid,"accent");else if(de.data.kind==="task"){let Ie=ce.find(Se=>Se.agent.tier==="orchestrator");Ie&&z(Ie.agent.cid,de.cid,"accent")}break}default:break}}),Eo=Mt("delta",q=>{let de=we.get(q.cid);de&&me(de,"thinking\u2026")}),gs=Mt("conversation",q=>{let de=q;z(de.from_cid,de.to_cid,"ok")}),Bl=q=>{let de=Math.min(.05,(q-Je)/1e3);Je=q;let xe=q/1e3;L(de,q),Z(q),O(xe),w(xe),m.animate(xe);for(let K of Oe)K(xe);g.update(),i.render(l,u),ue(),ge()};return i.setAnimationLoop(Bl),()=>{ot.abort(),i.setAnimationLoop(null),_t.disconnect(),Fl(),Ol(),Eo(),gs(),g.dispose();for(let q of[...ce])F(q);ce=[],we.clear();for(let q of pe.splice(0))X(q);Er(h),Er(M),Er(P),Er(f),Er(m.group),k.dispose(),D.dispose(),T.dispose(),E.dispose(),U.dispose(),p.dispose();for(let q of Object.values(S))q.dispose();i.dispose(),n.classList.remove("floor-view"),document.body.classList.remove("floor-mode"),n.replaceChildren()}}var wf="http://www.w3.org/2000/svg";function Zt(n,e,t={}){let i=document.createElementNS(wf,e);for(let r of Object.keys(t))i.setAttribute(r,String(t[r]));return n.appendChild(i),i}function Vn(n,e,t,i,r,s="var(--sheet)",o={}){return Zt(n,"rect",{x:e,y:t,width:i,height:r,fill:s,...o})}function Ll(n,e,t){e.forEach(([i,r],s)=>{let o=Zt(n,"circle",{cx:i,cy:r,r:1.6,fill:t?"var(--ok)":"var(--paper)","stroke-width":1});t&&s===0&&Zt(o,"animate",{attributeName:"opacity",values:"1;0.15;1",dur:"1.6s",repeatCount:"indefinite"})})}function py(n,e){Vn(n,8,18,68,44),Vn(n,13,23,40,14,"var(--paper)");for(let t of[17,21,25])Zt(n,"line",{x1:t,y1:27,x2:t,y2:33});Vn(n,13,41,40,15,"var(--paper)");for(let t of[45,48,51,54])Zt(n,"line",{x1:17,y1:t,x2:49,y2:t,"stroke-width":1}),Zt(n,"line",{x1:58,y1:t,x2:73,y2:t,"stroke-width":1});return Ll(n,[[58,30],[63,30],[68,30],[73,30]],e),76}function my(n,e){Vn(n,26,8,42,76),Zt(n,"polygon",{points:"68,8 76,3 76,79 68,84",fill:"var(--paper)"});for(let t of[30,36,42])Zt(n,"line",{x1:70,y1:t-1.25,x2:75,y2:t-4.4,"stroke-width":1});Vn(n,32,14,30,18,"var(--paper)"),Zt(n,"line",{x1:36,y1:22,x2:58,y2:22,"stroke-width":1}),Zt(n,"line",{x1:36,y1:26,x2:58,y2:26,"stroke-width":1});for(let t of[70,73,76,79])Zt(n,"line",{x1:34,y1:t,x2:62,y2:t,"stroke-width":1});return Zt(n,"circle",{cx:52,cy:60,r:3.5,fill:"var(--paper)"}),Ll(n,[[36,60],[42,60]],e),76}function gy(n,e){let t="#5a9a78";Vn(n,10,30,70,40,"var(--sheet)",{stroke:t}),[[14,34],[76,34],[14,66],[76,66]].forEach(([r,s])=>{Zt(n,"circle",{cx:r,cy:s,r:1.5,stroke:t,"stroke-width":1})}),Vn(n,30,40,20,20,"var(--paper)"),Zt(n,"rect",{x:36,y:46,width:8,height:8,"stroke-width":1});for(let r=0;r<10;r++){let s=52+r*2.2;Zt(n,"circle",{cx:s,cy:34,r:.9,"stroke-width":.8}),Zt(n,"circle",{cx:s,cy:37,r:.9,"stroke-width":.8})}return Vn(n,69,48,11,9,"var(--paper)"),Vn(n,69,58,11,9,"var(--paper)"),Vn(n,54,60,13,10,"var(--paper)"),Ll(n,[[18,62],[23,62]],e),80}function xy(n,e){Vn(n,10,30,66,30,"var(--sheet)",{rx:8}),Zt(n,"line",{x1:18,y1:35,x2:66,y2:35,"stroke-width":1});for(let t of[50,53])Zt(n,"line",{x1:20,y1:t,x2:50,y2:t,"stroke-width":1});return Ll(n,[[66,45]],e),76}var vy={server:py,workstation:my,edge:gy,apple:xy};function _y(n,e){let t=Zt(n,"g");Zt(t,"line",{x1:e,y1:46,x2:86,y2:46}),Vn(t,86,36,28,24),Zt(t,"circle",{cx:98,cy:46,r:6}),Zt(t,"circle",{cx:98,cy:46,r:1.5}),Vn(t,90,56,20,3,"var(--gold, #c9a227)",{stroke:"none"})}function Ef(n,e){let t=document.createElementNS(wf,"svg");t.setAttribute("viewBox","0 0 120 90"),t.setAttribute("class","device"),t.setAttribute("role","img"),t.setAttribute("aria-label",n),t.setAttribute("fill","none"),t.setAttribute("stroke","currentColor"),t.setAttribute("stroke-width","1.5");let i=vy[n](t,e.active);return e.gpu&&_y(t,i),t}var yy="http://www.w3.org/2000/svg",by=2e3,Cf={server:"Server",workstation:"Workstation",edge:"Edge",apple:"Apple"},Tf=[["Run",!1],["Role",!1],["State",!1],["VMs",!0],["Started",!0],["",!1]];function Au(n){return/run|ready|up|active/i.test(n)?"ok":/start|boot|pend|creat|launch/i.test(n)?"info":/fail|err|crash|kill|dead/i.test(n)?"danger":"muted"}var Pf=n=>n.some(e=>e.vms.some(t=>Au(t.state)==="ok")),My=n=>(n.runs??[]).reduce((e,t)=>e+t.vms.length,0),Tu=n=>(n/1024).toFixed(1),If=(n,e,t)=>Math.min(t,Math.max(e,n));function Af(n,e){let t=document.createElementNS(yy,n);for(let[i,r]of Object.entries(e))t.setAttribute(i,r);return t}function Sy(n){if(n<60)return`${Math.floor(n)}s`;let e=Math.floor(n/60);if(e<60)return`${e}m`;let t=Math.floor(e/60);return t<48?`${t}h ${e%60}m`:`${Math.floor(t/24)}d ${t%24}h`}function wy(n){let e=`${Cf[n.kind]} \xB7 ${n.cpus}-core ${n.arch} on ${n.backend.toUpperCase()} \xB7 ${Math.round(n.memory_mib/1024)} GiB memory`;return n.gpus.length?`${e} \xB7 ${n.gpus.map(t=>t.name).join(", ")}`:e}function Ey(n){let i=document.createElement("canvas");i.className="mach-spark",i.width=240,i.height=72,i.setAttribute("aria-hidden","true");let r=i.getContext("2d");if(!r)return i;let s=getComputedStyle(document.documentElement);r.strokeStyle=s.getPropertyValue("--rule").trim()||"#999",r.lineWidth=1,r.beginPath(),r.moveTo(0,72-.5),r.lineTo(240,72-.5),r.stroke();let o=n.slice(-60);return o.length<2||(r.strokeStyle=s.getPropertyValue("--ink").trim()||"#111",r.lineWidth=2,r.beginPath(),o.forEach((a,l)=>{let c=l/(o.length-1)*240,d=68-If(a.cpu_percent,0,100)/100*64;l===0?r.moveTo(c,d):r.lineTo(c,d)}),r.stroke()),i}function Ty(n){let e=x("div.mach-bar-fill",null);return e.style.width=`${If(n.mem_used_mib/Math.max(n.mem_total_mib,1)*100,0,100)}%`,x("div.mach-bar",null,e)}function kl(n,e,t,i){let r=x("div.mach-figure",null,e);return e.length>8&&r.classList.add("long"),x("div.mach-tally",null,x("div.mach-tally-label",null,n),r,i??null,t===null?null:x("div.mach-caption",null,t))}function Rf(n,e,t,i){let r=Af("svg",{class:"mach-wires",viewBox:"0 0 100 100",preserveAspectRatio:"none","aria-hidden":"true"}),s=x("div.mach-wires-box",null,r),o=x("div.mach-fleet");o.style.gridTemplateRows=`repeat(${Math.max(e.length,1)}, minmax(44px, 1fr))`,e.length||o.append(pt("machine","No machines.")),e.forEach((l,c)=>{let d=Pf(l.runs??[]),m=(c+.5)/e.length*100;r.append(Af("line",{x1:"0",y1:"50",x2:"100",y2:String(m),class:d?"mach-wire live":"mach-wire","vector-effect":"non-scaling-stroke"}));let u=x("div.mach-wire-label",null,`${l.agents??0} agents \xB7 ${My(l)} VMs`);u.style.top=`${(50+m)/2}%`,s.append(u);let g=x("span.mach-mark",null);d&&g.classList.add("live"),l.reachable===!1&&g.classList.add("down");let _=l.reachable===!1?"unreachable":d?"running":"idle",v=x("div.mach-block",{role:"button",tabindex:"0"},x("div.mach-row",null,g,x("span.mach-name",null,l.name)),x("div.faint",null,`${Cf[l.kind]} \xB7 ${_}`));v.classList.toggle("selected",l.id===t),v.addEventListener("click",()=>i(l.id)),v.addEventListener("keydown",f=>{f.key==="Enter"&&i(l.id)}),o.append(v)});let a=x("div.mach-plane",null,x("div.mach-caps",null,"Control plane"),x("div.faint",null,n?`${n.agents.total} agents \xB7 ${n.agents.working} working`:"Unreachable"),x("div.mach-chips",null,lt("apid",n?"ok":"danger",!0),lt("hub",n?.hub?"ok":"muted",!0)));return x("div.mach-topo",null,a,s,o)}function Lf(n,e){let t=x("div"),i=x("div");n.append(x("div.view-head",null,x("h1",null,"Machines")),t,i);let r=new Set,s=new Set,o=new Set,a=new Map,l=!0,c,d=[],m=null,u=null,g=(p,S)=>S.length?x("div.vms",null,S.map(y=>{let R=`${p}/${y.index}`,I=null;if(y.serial){let k=x("details",null,x("summary",null,"Serial"),x("pre.pre",null,y.serial));k.open=s.has(R),k.addEventListener("toggle",()=>k.open?s.add(R):s.delete(R)),I=k}return x("div.vm",null,x("div.row",null,x("strong",null,`VM ${y.index}`),lt(y.state,Au(y.state),!0),x("span.faint",null,`guest CID ${y.guest_cid??"\u2014"}`)),I)})):x("p.faint",null,"No VMs."),_=p=>{let S=a.get(p.id),y=x("td",{colspan:Tf.length},S?g(p.id,S.vms):x("p.faint",null,"Loading\u2026")),R=x("tr.detail",null,y);return He.run(p.id).then(I=>{a.set(p.id,I),R.isConnected&&Be(y,g(p.id,I.vms))},I=>{R.isConnected&&Be(y,x("p.faint",null,I instanceof Error?I.message:String(I)))}),R},v=(p,S)=>{let y=S.nextElementSibling;y&&y.classList.contains("detail")?(y.remove(),r.delete(p.id)):(r.add(p.id),S.after(_(p)))},f=async p=>{await mt(()=>He.killRun(p),`Killed run ${p}`),l&&w()},h=p=>{let S=x("tbody");for(let y of p){let R=Ye("Kill",k=>{k.stopPropagation(),f(y.id)},{kind:"danger",size:"sm",icon:"stop"}),I=x("tr.clickable",null,x("td",null,y.id),x("td",null,y.role??"fleet"),x("td",null,lt(y.state,Au(y.state),!0)),x("td",{class:"num"},`${y.vms.length} VMs`),x("td",{class:"num"},`started ${qt(y.created_at)}`),x("td",{class:"num"},R));I.addEventListener("click",()=>v(y,I)),S.append(I),r.has(y.id)&&S.append(_(y))}return x("table.table.mach-runs",null,x("thead",null,x("tr",null,Tf.map(([y,R])=>x("th",{class:R?"num":""},y)))),S)},M=p=>{let S=p.runs??[],y=Ef(p.kind,{gpu:p.gpus.length>0,active:Pf(S)});y.classList.add("mach-dev");let R=p.stats,I=R?x("div.mach-live",null,x("div.mach-tallies",null,kl("CPU",`${Math.round(R.cpu_percent)}%`,null,Ey(R.history)),kl("Memory",`${Tu(R.mem_used_mib)} / ${Tu(R.mem_total_mib)}`,"GiB used of GiB",Ty(R)),kl("Load",`${R.load[0].toFixed(1)} \xB7 ${R.load[1].toFixed(1)} \xB7 ${R.load[2].toFixed(1)}`,"1 / 5 / 15 min"),kl("Agents' VMs",String(R.vms_running),`${Tu(R.guest_mem_mib)} GiB reserved`)),x("p.faint",null,`Up ${Sy(R.uptime_s)}`)):x("p.mach-none",null,"No live stats."),k=x("dl.kv"),D=[["Class",p.class],["Arch",p.arch],["Backend",p.backend],["Irqchip",p.irqchip],["vsock",p.vsock_mode],["noded port",p.noded_port===void 0?"\u2014":String(p.noded_port)]];for(let[H,$]of D)k.append(x("dt",null,H),x("dd",null,$));let N=x("details.mach-tech",null,x("summary",null,"Technical details"),k);return N.open=o.has(p.id),N.addEventListener("toggle",()=>N.open?o.add(p.id):o.delete(p.id)),x("section.mach-section",{id:`mach-${p.id}`},x("div.mach-figure-box",null,y),x("div.mach-body",null,x("h2.mach-heading",null,p.name),x("p.mach-sentence",null,wy(p)),I,x("div.mach-caps",null,"Runs"),S.length?h(S):x("p.faint",null,"No runs."),N))},P=p=>{u=p,t.replaceChildren(Rf(m,d,u,P)),document.getElementById(`mach-${p}`)?.scrollIntoView({behavior:"smooth",block:"start"})},w=async()=>{try{let[p,S]=await Promise.all([He.overview().catch(()=>null),He.machines()]);if(!l)return;m=p,d=S,S.some(R=>R.id===u)||(u=S[0]?.id??null);let y=new Set(S.flatMap(R=>(R.runs??[]).map(I=>I.id)));for(let R of Array.from(r))y.has(R)||r.delete(R);t.replaceChildren(Rf(p,S,u,P)),i.replaceChildren(...S.length?S.map(M):[pt("machine","No machines.")])}catch(p){if(!l)return;t.replaceChildren(),i.replaceChildren(pt("alert","Machines unreachable",p instanceof Error?p.message:String(p)))}},T=()=>{c!==void 0&&clearTimeout(c),c=setTimeout(()=>{c=void 0,w()},500)},E=[Mt("run",T),Mt("vm",T)],U=setInterval(()=>{w()},by);return w(),()=>{l=!1,E.forEach(p=>p()),clearInterval(U),c!==void 0&&clearTimeout(c)}}var Ay=["default","low","medium","high","xhigh","max"],kf="models-policy-known";function Df(n){let e=!0;return Be(n,x("p.faint",null,"Loading\u2026")),mt(()=>Promise.all([He.settings(),He.models()])).then(t=>{if(e){if(!t){Be(n,pt("alert","Models unavailable","apid did not answer. Reload to try again."));return}Ry(n,t[0],t[1])}}),()=>{e=!1}}function Ry(n,e,t){let i=e,r=Pu(e),s=e.dev_models===!0,o=new Set(t.models.map(p=>`${p.provider}/${p.id}`)),a=new Set,l=!1,c=Ye("Save",()=>{_()},{kind:"primary",icon:"check"}),d=x("datalist",{id:kf},...t.models.map(p=>x("option",{value:`${p.provider}/${p.id}`},p.name??`${p.provider}/${p.id}`))),m=x("div.stack.models-policy");Be(n,x("div.policy-bar",null,c),d,m);function u(){c.disabled=a.size>0||JSON.stringify(r)===JSON.stringify(i)}function g(){a.clear(),Be(m,f(),P(),T(),U()),u()}async function _(){c.disabled=!0;let p=await mt(()=>He.saveSettings(r),"Policy saved");p?(i=Pu(p),r=Pu(p),g()):u()}function v(p,S,y){y?a.add(S):a.delete(S),p.classList.toggle("invalid",y),u()}function f(){return Gt("Model policy",x("div.stack",null,x("p.policy-lead",null,"Agents may only run the models on their tier's list."),x("div.policy-cols",null,h("orchestrator","Orchestrator"),h("worker","Workers"))),{actions:s?[lt("Development models enabled \u2014 policy relaxed","warn")]:[]})}function h(p,S){let R=r.model_policy[p].map((D,N)=>M(p,D,N)),I=Jt({placeholder:"provider/model",list:kf,autocomplete:"off","aria-label":`${S}: model to add`}),k=()=>{let D=I.value.trim();if(D!==""){if(!/^[^/\s]+\/\S+$/.test(Ru(D).base)){rn("Use provider/model, with an optional @effort","error");return}r.model_policy[p].includes(D)||(r.model_policy[p]=[...r.model_policy[p],D]),g()}};return I.addEventListener("keydown",D=>{D.key==="Enter"&&(D.preventDefault(),k())}),x("section.policy-col",null,x("h3",null,S),x("ul.policy-list",null,...R.length?R:[x("li.faint",null,"No models")]),x("div.policy-add",null,I,Ye("Add model",k,{size:"sm"})))}function M(p,S,y){let{base:R,effort:I}=Ru(S),k=o.has(R),D=li(k?"ok":"muted");D.title=k?"Listed by its provider":"Not listed by its provider";let N=I===null?null:lt("effort pinned","accent");N&&(N.title=`@${I}`);let H=Ye("",()=>{r.model_policy[p]=r.model_policy[p].filter(($,ee)=>ee!==y),g()},{kind:"ghost",size:"sm",icon:"x",title:`Remove ${S}`});return x("li.policy-row",null,D,x("span.mono",{title:S},R),N,H)}function P(){let p=pn(Cu(r.model_policy.orchestrator,r.orchestrator_model),r.orchestrator_model);p.addEventListener("change",()=>{r.orchestrator_model=p.value,u()});let S=pn(Cu(r.model_policy.worker,r.default_model),r.default_model);S.addEventListener("change",()=>{r.default_model=S.value,u()});let y=Array.from(new Set([...ft.get().filter(I=>I.tier!=="orchestrator"&&I.role!=="orchestrator").map(I=>I.role),...Object.keys(r.role_models)])).sort(),R=y.length?or([{label:"Role",cell:I=>x("span.mono",null,I)},{label:"Model",cell:I=>w(I)}],y):pt("agents","No roles connected");return Gt("Defaults",x("div.stack",null,yt("Orchestrator model",p),yt("Worker default",S),x("h3",null,"Per role"),R))}function w(p){let S=r.role_models[p]??"",y=pn(Cu(r.model_policy.worker,S===""?null:S,{value:"",label:"Worker default"}),S);return y.addEventListener("change",()=>{y.value===""?delete r.role_models[p]:r.role_models[p]=y.value,u()}),y}function T(){let p=E("max_turns",1,200,r.max_turns,I=>{r.max_turns=I}),S=E("max_tokens",0,1e9,r.max_tokens,I=>{r.max_tokens=I}),y=pn(Ay.map(I=>({value:I,label:I})),r.effort??"default");y.addEventListener("change",()=>{r.effort=y.value==="default"?null:y.value,u()});let R=Kn({rows:5,spellcheck:"false",placeholder:"api.example.com"});return R.value=r.fetch_allow.join(`
`),R.addEventListener("input",()=>{r.fetch_allow=R.value.split(`
`).map(I=>I.trim()).filter(I=>I!==""),u()}),Gt("Limits",x("div.stack",null,yt("Max turns",p),yt("Max tokens (0 = provider default)",S),yt("Effort",y),yt("Fetch allowlist",R)))}function E(p,S,y,R,I){let k=Jt({type:"number",min:S,max:y,step:1,value:R});return k.addEventListener("input",()=>{let D=Number(k.value),N=k.value.trim()!==""&&Number.isInteger(D)&&D>=S&&D<=y;N&&I(D),v(k,p,!N)}),k}function U(){let p=r.model_policy.orchestrator.concat(r.model_policy.worker).map(k=>Ru(k).base),S=Array.from(new Set([...p,...Object.keys(r.prices)])).sort(),y=(k,D)=>{let N=r.prices[k],H=Jt({type:"number",min:0,step:"any",value:N?N[D]:0});return H.addEventListener("input",()=>{let $=Number(H.value),ee=H.value.trim()!==""&&Number.isFinite($)&&$>=0;if(ee){let J=r.prices[k]??{input:0,output:0};J[D]=$,r.prices[k]=J}v(H,`price:${k}:${D}`,!ee)}),H},R=S.length?or([{label:"Model",cell:k=>x("span.mono",null,k)},{label:"Input (credits / 1M)",num:!0,cell:k=>y(k,"input")},{label:"Output (credits / 1M)",num:!0,cell:k=>y(k,"output")}],S):pt("models","No models listed"),I=x("details.prices",null,x("summary",null,"Models"),R);return I.open=l,I.addEventListener("toggle",()=>{l=I.open}),Gt("Prices",I)}g()}function Ru(n){let e=n.lastIndexOf("@");return e>0?{base:n.slice(0,e),effort:n.slice(e+1)}:{base:n,effort:null}}function Cu(n,e,t){let r=(e!==null&&e!==""&&!n.includes(e)?[...n,e]:n).map(s=>({value:s,label:n.includes(s)?s:`${s} (not on the list)`}));return t?[t,...r]:r}function Pu(n){return JSON.parse(JSON.stringify(n))}var Cy=["Configured with a key","Needs a key","Local"],Py={env:["Key set","ok"],secret:["Key set","ok"],none:["No key needed","muted"],command:["Command","info"],missing:["Key missing","warn"]};function Nf(n,e){return e.find(t=>t.id===n.id)??e.find(t=>t.kind===n.kind)}function Iy(n){return n.credential==="missing"?"Needs a key":n.credential==="none"?"Local":"Configured with a key"}function Ly(n){return n.split(",").map(e=>e.trim()).filter(Boolean)}function Uf(n){let e=[],t=[],i=x("div.prov-body",null,x("p.faint",null,"Loading providers\u2026")),r=Ye("Add provider",()=>_(),{kind:"primary"});n.append(x("div.prov-bar",null,r),i);let s=async()=>{let[v,f]=await Promise.all([mt(()=>He.providerKinds()),mt(()=>He.providers())]);v&&(e=v),f&&(t=f),o()},o=()=>{if(!t.length){Be(i,pt("models","No providers yet"));return}let v={"Configured with a key":[],"Needs a key":[],Local:[]};for(let h of t)v[Iy(h)].push(h);let f=Cy.filter(h=>v[h].length>0).map(h=>x("section.prov-group",null,x("h2.prov-group-title",null,h),x("div.prov-grid",null,v[h].map(a))));Be(i,...f)},a=v=>{let f=Nf(v,e),[h,M]=Py[v.credential],P=x("div.prov-models");P.hidden=!0;let w=!1,T=Ye("",async()=>{if(P.hidden=!P.hidden,P.hidden||w)return;let E=await mt(()=>He.providerModels(v.id));E&&(w=!0,Be(P,E.length?x("ul.prov-model-list",null,E.map(U=>x("li.mono",null,U.id))):x("p.faint",null,"No models listed")))},{size:"sm",kind:"ghost",icon:"models",title:"Models"});return x("article.prov-card",null,x("header.prov-head",null,ql(f?.brand??"mock",!0),x("div.prov-title",null,x("h3.truncate",null,f?.label??v.id),x("span.faint.mono.truncate",{title:v.base_url??""},`${v.id} \xB7 ${v.base_url??"\u2014"}`))),x("div.prov-meta",null,lt(h,M,!0),x("span.faint",null,`${v.models.length} ${v.models.length===1?"model":"models"}`)),x("div.prov-actions",null,Ye("Test",()=>{l(v)},{size:"sm"}),Ye(v.credential==="missing"?"Add key":"Key",()=>c(v),{size:"sm"}),x("span.prov-spacer"),T,Ye("",()=>u(v),{size:"sm",kind:"ghost",icon:"edit",title:"Edit"}),Ye("",()=>d(v),{size:"sm",kind:"ghost",icon:"trash",title:"Remove"})),P)},l=async v=>{let f=await mt(()=>He.testProvider(v.id));f&&(f.ok?rn(`OK \xB7 ${f.latency_ms} ms \xB7 ${f.model}`):rn(f.error??"Test failed","error"))},c=v=>{let f=()=>{},h=Jt({type:"password",autocomplete:"new-password",placeholder:"API key"}),M=Ye("Save key",async()=>{let P=h.value.trim();if(!P)return;M.disabled=!0;let w=await mt(()=>He.setSecret(v.id,P),"Key saved");M.disabled=!1,w!==void 0&&(h.value="",f(),await s())},{kind:"primary"});h.addEventListener("keydown",P=>{P.key==="Enter"&&M.click()}),f=Ln(`Set key \xB7 ${v.id}`,x("div.stack",null,yt("API key",h),x("div.prov-row",null,x("span.prov-spacer"),M)))},d=v=>{let f=()=>{},h=Ye("Remove provider",async()=>{await mt(()=>He.removeProvider(v.id),`Removed ${v.id}`)!==void 0&&(f(),await s())},{kind:"danger"}),M=Ye("Cancel",()=>f(),{});f=Ln(`Remove ${v.id}?`,x("div.stack",null,x("div.prov-row",null,x("span.prov-spacer"),M,h)))},m=(v,f,h,M)=>{let P=()=>{},w=Jt({value:f.id,placeholder:"provider id",autocomplete:"off",disabled:!f.idEditable}),T=Jt({value:f.base_url,placeholder:"https://",autocomplete:"off"}),E=typeof f.auth=="object"&&"env"in f.auth?f.auth.env:null,U=E===null?null:Jt({value:E,placeholder:"ENV_VAR_NAME",autocomplete:"off"}),p=f.models===null?null:Jt({value:f.models.join(", "),autocomplete:"off"}),S=f.keyField?Jt({type:"password",autocomplete:"new-password",placeholder:"Optional"}):null,y=[yt("Provider id",w),yt("Base URL",T)];U&&y.push(yt("Auth env var",U)),p&&y.push(yt("Models",p,"Comma-separated")),S&&y.push(yt("API key",S,"Optional"));let R=Ye(h,async()=>{let I=w.value.trim();if(!I){rn("Give the provider an id","error");return}R.disabled=!0;let k=await M({id:I,base_url:T.value.trim()||null,auth:U?{env:U.value.trim()}:f.auth,models:p?Ly(p.value):null,key:S?.value.trim()??""});R.disabled=!1,k&&P()},{kind:"primary"});P=Ln(v,x("div.stack",null,f.notes?x("p.faint",null,f.notes):null,...y,x("div.prov-row",null,x("span.prov-spacer"),R)))},u=v=>{let f=Nf(v,e);m(`Edit ${v.id}`,{id:v.id,base_url:v.base_url??"",auth:v.auth,models:v.models,idEditable:!1,keyField:!1,notes:f?.notes},"Save",async h=>await mt(()=>He.updateProvider({id:v.id,kind:v.kind,base_url:h.base_url,auth:h.auth,models:h.models??v.models,options:v.options??void 0,extra_headers:v.extra_headers}),"Saved")===void 0?!1:(await s(),!0))},g=v=>{m(v.label,{id:v.id,base_url:v.base_url??"",auth:v.auth,models:null,idEditable:!0,keyField:!0,notes:v.notes},"Add provider",async f=>await mt(()=>He.addProvider({id:f.id,kind:v.kind,base_url:f.base_url,auth:f.auth,models:v.models,options:v.options}),`Added ${f.id}`)===void 0?!1:(f.key&&await mt(()=>He.setSecret(f.id,f.key),"Key saved"),await s(),!0))},_=()=>{let v=()=>{},f=new Set(t.map(E=>E.id)),h=e.filter(E=>!f.has(E.id)),M=Jt({type:"search",placeholder:"Search providers",autocomplete:"off"}),P=x("div.prov-kind-grid"),w=E=>{v(),g(E)},T=()=>{let E=M.value.trim().toLowerCase(),p=h.filter(S=>!E||`${S.label} ${S.id} ${S.kind}`.toLowerCase().includes(E)).map(S=>{let y=x("button.prov-kind",{type:"button"},ql(S.brand,!0),x("span.truncate",null,S.label));return y.addEventListener("click",()=>w(S)),y});Be(P,...p.length?p:[x("p.faint",null,h.length?"No matches":"Every provider is already configured")])};M.addEventListener("input",T),v=Ln("Add provider",x("div.stack",null,M,P)),T()};return s(),()=>{}}function Ff(n,e){let t=e[0]==="policy"?"policy":"providers",i=x("div.stack");return n.append(x("div.view-head",null,x("h1",null,"Models")),qu([{key:"providers",label:"Providers"},{key:"policy",label:"Policy & defaults"}],t,r=>Mn("models",...r==="policy"?["policy"]:[])),i),t==="policy"?Df(i):Uf(i)}var ky=40;function Of(n){let e=x("div.grid.cols-4"),t=x("div"),i=x("ol.feed"),r=x("div");n.append(x("div.view-head",null,x("h1",null,"Overview")),e,x("div.grid.cols-2.overview-main",null,t,Gt("Activity",i,{flush:!0})),Gt("Fleet",r,{flush:!0}));let s=[sr.watch(o=>{if(!o)return Be(e,pt("alert","apid is unreachable","Start apid, then reload."));let a=(o.tasks.open??0)+(o.tasks.claimed??0);Be(e,Zn("Agents",o.agents.total,`${o.agents.working} working \xB7 ${o.agents.offline} offline`),Zn("Open tasks",a,`${o.tasks.done??0} done`),Zn("Worker model",o.default_model.split("/").pop()??"\u2014",o.default_model.split("/")[0]),Zn("Orchestrator model",o.orchestrator_model.split("/").pop()??"\u2014",o.orchestrator_model.split("/")[0]))}),ft.watch(o=>{Dy(t,o.find(a=>a.tier==="orchestrator")),Ny(r,o)}),Mt("transcript",o=>Uy(i,o))];return Be(i,x("li.feed-empty.faint",null,"Waiting for activity\u2026")),()=>s.forEach(o=>o())}function Dy(n,e){if(!e){Be(n,Gt("Orchestrator",pt("orchestrator","No orchestrator connected","Add the orchestrator role to the fleet to delegate work by goal.")));return}let t=Kn({placeholder:"Tell the orchestrator what you want done\u2026",rows:3}),i=Ye("Send",()=>r(),{kind:"primary",icon:"send"}),r=async()=>{let o=t.value.trim();if(!o)return;i.disabled=!0;let a=await mt(()=>He.chat(e.cid,o),"Sent to the orchestrator");i.disabled=!1,a&&(t.value="")};t.addEventListener("keydown",o=>{o.key==="Enter"&&(o.metaKey||o.ctrlKey)&&r()});let s=e.status==="working"?lt("Working","info",!0):e.status==="idle"?lt("Idle","ok",!0):lt("Offline","muted",!0);Be(n,Gt(x("div.row",null,ai("orchestrator"),x("h2",null,mn(e)),x("span.faint",null,Sn(e.role)),s),x("div.stack",null,x("p.muted",null,`Plans the work and delegates it to the fleet \xB7 ${e.model}`),t,x("div.row",null,x("span.faint",null,"Ctrl+Enter to send"),x("span",{style:"flex:1"}),x("a",{href:bn("agents",e.cid)},"Open transcript"),i))))}function Ny(n,e){if(!e.length)return Be(n,pt("agents","No agents yet","Agents appear here as their VMs connect."));Be(n,x("div.fleet",null,e.map(t=>{let i=t.status==="working"?"info":t.status==="idle"?"ok":"muted",r=x("button.fleet-tile",{type:"button",title:`${mn(t)} \xB7 ${Sn(t.role)} \xB7 ${t.model}`},x("span.row",null,x("span.dot",{class:i}),x("strong.truncate",null,mn(t))),x("span.faint.truncate",null,t.work?jt(t.work.prompt,60):t.status==="offline"?"offline":Sn(t.role)));return r.addEventListener("click",()=>Mn("agents",t.cid)),r})))}function Uy(n,e){let t=Fy(e);if(!t)return;n.querySelector(".feed-empty")?.remove();let i=ft.get().find(o=>o.cid===e.cid),r=i?mn(i):`agent ${e.cid}`,s=x("li",null,x("span.feed-time.faint",null,qt(e.at_ms)),x("a",{href:bn("agents",e.cid)},r),x("span.muted",null,t));for(n.prepend(s);n.children.length>ky;)n.lastElementChild?.remove()}function Fy(n){let e=n.data;switch(n.kind){case"work_start":return`started ${String(e.kind)}: ${jt(String(e.prompt??""),80)}`;case"work_end":return`finished (${String(e.status)})`;case"tool_request":return`called ${String(e.name)}`;case"error":return`error: ${jt(String(e.message??""),80)}`;default:return null}}var Oy=[{label:"Open",idle:"Nothing open."},{label:"In progress",idle:"Nothing in progress."},{label:"Done",idle:"Nothing finished yet."},{label:"Failed or withdrawn",idle:"Nothing failed or withdrawn."}],By=new Set(["failed","withdrawn","cancelled"]),zy=new Set(["work_start","tool_request","work_end","error"]);function Dl(n){return n==="open"?0:n==="done"?2:By.has(n)?3:1}function Hy(n){switch(Dl(n)){case 0:return"info";case 1:return"accent";case 2:return"ok";default:return n==="withdrawn"?"muted":"danger"}}var ms=n=>typeof n=="string"||typeof n=="number"?String(n):"",Bf=n=>n.split(`
`).find(e=>e.trim())?.trim()??"Untitled task",Iu=n=>n.replace(/_/g," ").replace(/^./,e=>e.toUpperCase()),Gy=n=>{let e=ft.get().find(t=>t.cid===n);return e?mn(e):`agent ${n}`},zf=()=>ft.get().filter(n=>n.status==="idle"&&n.tier==="worker"),Vy=n=>ft.get().find(e=>e.cid===n)?.status??"offline",Nl=n=>String(n).padStart(2,"0"),Hf=n=>{let e=Math.max(0,Math.floor(n/1e3));return`${Nl(Math.floor(e/60))}:${Nl(e%60)}`},Wy=n=>{let e=Math.max(0,Math.round(n/1e3));return e<60?`${e}s`:e<3600?`${Math.floor(e/60)}m ${Nl(e%60)}s`:`${Math.floor(e/3600)}h ${Nl(Math.floor(e%3600/60))}m`},Xy=n=>new Date(n).toLocaleTimeString([],{hour:"2-digit",minute:"2-digit",second:"2-digit"}),Lu=n=>ms(n.data.tool)||ms(n.data.name)||"tool";function Vf(n){let e=n.data.input??{};return Lu(n).toLowerCase()==="bash"&&typeof e.command=="string"?e.command:null}function qy(n){switch(n.kind){case"tool_request":{let e=Vf(n);return e!==null?`$ ${e}`:`\u2192 ${Lu(n)}`}case"model_request":return"thinking\u2026";case"work_start":return"starting\u2026";case"work_end":return"wrapping up\u2026";default:return null}}function $y(n){switch(n.kind){case"work_start":return`Started \xB7 ${ms(n.data.kind)||"work"}`;case"tool_request":{let e=Vf(n);return e!==null?`$ ${e}`:`\u2192 ${Lu(n)}`}case"work_end":{let e=ms(n.data.status);return e?`Finished \xB7 ${e}`:"Finished"}default:return ms(n.data.message)||ms(n.data.error)||"Error"}}function Yy(n,e){let t=new Set(e.filter(i=>i.kind==="work_start"&&((i.data.origin??{}).task===n||i.data.task===n)).map(i=>i.work));return e.filter(i=>t.has(i.work)&&zy.has(i.kind))}function Gf(n){if(n===null)return x("span.faint",null,"Unassigned");let e=x("a",{href:bn("agents",n)},Gy(n));return e.addEventListener("click",t=>t.stopPropagation()),e}var Zy=n=>x("li",{class:`k-${n.kind}`},x("time",null,Xy(n.at_ms)),x("span",null,$y(n)));function Wf(n,e){let t=e[0]??"",i=[],r=null,s=null,o=!1,a,l=0,c,d=new Map,m=new Map,u=new Set,g=[],_=()=>{Be(n,t?U():E())},v=async y=>{if(s=null,y.agent_cid===null){s=[];return}try{s=Yy(y.id,await He.transcript(y.agent_cid))}catch{s=[]}},f=async()=>{let y=++l;try{if(t){let R=await He.task(t);if(y!==l||(r=R,o=!1,await v(R),y!==l))return}else{let R=await He.tasks();y===l&&(i=R)}}catch{y===l&&t&&(r=null,o=!0)}y===l&&_()},h=()=>{clearTimeout(a),a=setTimeout(()=>{f()},300)},M=()=>{let y=Jt({required:!0,placeholder:"What should get done?"}),R=Kn({rows:4}),I=Kn({rows:3}),k=Array.from(new Set(ft.get().filter(ie=>ie.tier==="worker").map(ie=>ie.role).filter(Boolean))).sort(),D=pn([{value:"",label:"Any"},...k.map(ie=>({value:ie,label:ie}))]),N=Jt({type:"number",min:0,step:1,value:100}),H=pn([{value:"",label:"Let the scheduler choose"},...zf().map(ie=>({value:String(ie.cid),label:mn(ie)}))]),$=x("button.btn.primary",{type:"submit"},ai("plus"),"Create task"),ee=x("form.stack",null,yt("Title",y),yt("Details",R),yt("Acceptance",I),x("div.grid.cols-2",null,yt("Role",D),yt("Budget",N)),yt("Assign to",H),x("div.row",null,x("span",{style:"flex:1"}),$));ee.addEventListener("submit",async ie=>{ie.preventDefault();let ce=y.value.trim();if(!ce){y.focus();return}let we=R.value.trim();$.disabled=!0;let pe=await mt(()=>He.createTask({goal:we?`${ce}

${we}`:ce,acceptance:I.value.trim()||void 0,role:D.value||void 0,budget:N.value===""?100:Number(N.value),assignee:H.value?Number(H.value):void 0}),"Task created");$.disabled=!1,pe&&(J(),Mn("tasks",pe.id))});let J=Ln("New task",ee,()=>{c=void 0});c=J};function P(y){let R=D=>y.filter(N=>Dl(N.state)===D).length,I=ft.get().filter(D=>D.status==="working").length,k=(D,N)=>x("div.tk-fig",null,x("b",null,String(N)),x("span",null,D));return x("div.tk-summary",null,k("Open",R(0)),k("In progress",R(1)),k("Done",R(2)),k("Failed",R(3)),k("Agents working",I))}function w(y){let R=Dl(y.state),I=m.get(y.id),k=["task-card",`lane-${R}`];u.has(y.id)||k.push("enter"),I!==void 0&&I!==y.state&&k.push("changed"),u.add(y.id),m.set(y.id,y.state);let D=y.poster,N=x("div.task-who",null,x("span.task-dot",{class:`s-${Vy(y.agent_cid)}`}),Gf(y.agent_cid),D?x("span.faint",null,`by ${D}`):null),H;if(R===1){let J=x("span.task-elapsed",null,Hf(Date.now()-y.updated_at));g.push({el:J,since:y.updated_at});let ie=y.agent_cid!==null&&d.get(y.agent_cid)||"waiting for the first step\u2026";H=x("div.task-live",null,x("div.task-stripe"),x("div.task-activity",{"data-activity":String(y.agent_cid??"")},ie),x("div.task-meta",null,x("span.faint",null,"elapsed"),J))}else R===2?H=x("div",null,y.note?x("p.task-note",null,jt(y.note,120)):null,x("div.task-meta",null,x("span.faint",null,`took ${Wy(y.updated_at-y.created_at)}`),x("span.faint",null,qt(y.updated_at)))):R===3?H=x("div",null,x("p.task-reason",null,y.outcome||y.note||Iu(y.state)),x("div.task-meta",null,x("span.faint",null,Iu(y.state)),x("span.faint",null,qt(y.updated_at)))):H=x("div.task-meta",null,x("span.faint",null,`budget ${y.budget}`),x("span.faint",null,`created ${qt(y.created_at)}`));let $=x("div",{class:k.join(" "),role:"link",tabindex:0},x("div.task-top",null,x("span.task-id",null,y.id),x("span.task-role",null,y.role?Sn(y.role):"Any role")),x("strong.task-title",null,Bf(y.goal)),N,H),ee=()=>Mn("tasks",y.id);return $.addEventListener("click",ee),$.addEventListener("keydown",J=>{J.key==="Enter"&&ee()}),$}function T(y){g=[];let R=y.slice().sort((I,k)=>k.updated_at-I.updated_at);return x("div.tk-board-wrap",null,x("div.tk-board",null,Oy.map((I,k)=>{let D=R.filter(N=>Dl(N.state)===k);return x("section.tk-lane",{class:`lane-${k}`},x("header.tk-lane-head",null,x("span.tk-mark",{class:`mark-${k}`}),x("h2",null,I.label),x("span.tk-count",null,String(D.length))),x("div.tk-lane-body",null,D.length?D.map(w):x("p.tk-empty",null,I.idle)))})))}function E(){return x("div.tasks-view",null,x("div.view-head",null,x("h1",null,"Tasks"),x("div.actions",null,Ye("New task",M,{kind:"primary",icon:"plus"}))),P(i),T(i))}function U(){let y=x("a.back",{href:bn("tasks")},ai("chevronRight"),"Tasks");if(o)return x("div.tasks-view",null,y,x("p.tk-empty",null,"Task not found."));let R=r;if(!R)return x("div.tasks-view",null,y);let I=R.state==="open",k=pn([{value:"",label:"Assign to\u2026"},...zf().map(ie=>({value:String(ie.cid),label:mn(ie)}))],"");k.addEventListener("change",async()=>{k.value&&(k.disabled=!0,await mt(()=>He.assign(Number(k.value),R.id),"Assigned"),k.disabled=!1,f())});let D=Ye("Withdraw",async()=>{D.disabled=!0,await mt(()=>He.withdrawTask(R.id),"Task withdrawn")?f():D.disabled=!1},{kind:"danger",icon:"x"}),N=x("dl.kv",null,x("dt",null,"State"),x("dd",null,lt(Iu(R.state),Hy(R.state),!0)),x("dt",null,"Role"),x("dd",null,R.role?Sn(R.role):"Any role"),x("dt",null,"Assignee"),x("dd",null,Gf(R.agent_cid)),x("dt",null,"Budget"),x("dd",null,String(R.budget)),x("dt",null,"Created"),x("dd",null,qt(R.created_at)),x("dt",null,"Updated"),x("dd",null,qt(R.updated_at))),H=s,$=H===null?x("p.tk-empty",null,"Loading\u2026"):H.length===0?x("p.tk-empty",null,R.agent_cid===null?"Not yet assigned.":"No steps recorded yet."):x("ol.tk-tl",null,H.map(Zy)),ee=x("div.tk-main",null,x("section.tk-block",null,x("h3.tk-label",null,"Goal"),x("div.tk-goal",null,R.goal)),R.acceptance?x("section.tk-block",null,x("h3.tk-label",null,"Acceptance"),x("div.tk-prose",null,R.acceptance)):null,R.note?x("section.tk-block",null,x("h3.tk-label",null,"Result"),x("div.tk-result",null,R.note)):null),J=x("aside.tk-side",null,x("section.tk-block",null,x("h3.tk-label",null,"Facts"),N,I?k:null,I?x("div.row",null,x("span",{style:"flex:1"}),D):null),x("section.tk-block",null,x("h3.tk-label",null,"Timeline"),$));return x("div.tasks-view",null,y,x("div.view-head",null,x("div.tk-head",null,x("span.task-id",null,R.id),x("h1",null,Bf(R.goal)))),x("div.tk-cols",null,ee,J))}_();let p=setInterval(()=>{let y=Date.now();for(let R of g)R.el.textContent=Hf(y-R.since)},1e3),S=[Mt("task",h),Mt("work",h),Mt("transcript",y=>{let R=y;if(t){r&&R.cid===r.agent_cid&&h();return}let I=qy(R);I!==null&&(d.set(R.cid,I),n.querySelectorAll(`[data-activity="${R.cid}"]`).forEach(k=>{k.textContent=I}))}),ft.watch(()=>_())];return f(),()=>{clearTimeout(a),clearInterval(p),S.forEach(y=>y()),g=[],c?.()}}var xi=n=>n.toLocaleString("en-US"),ku=n=>n instanceof Error?n.message:String(n),Ky={mint:"ok",spend:"warn",payment:"accent",refund:"info",transfer:"info",fine:"danger"},Xf=n=>n.postings.reduce((e,t)=>t.amount>0?e+t.amount:e,0),Jy=(n,e)=>n.filter(t=>t.kind===e).reduce((t,i)=>t+Xf(i),0),jy=(n,e,t,i,r)=>{let s=(c,d,m,u)=>`<rect class="tr-box" x="${c}" y="${d}" width="170" height="70"/><text class="tr-label" x="${c+12}" y="${d+22}">${m}</text><text class="tr-fig" x="${c+12}" y="${d+54}">${xi(u)}</text>`,o=(c,d,m,u)=>`<line class="tr-line" x1="${c}" y1="${d}" x2="${m}" y2="${u}" marker-end="url(#tr-arrow)"/>`,a='<svg class="tr-flow" viewBox="0 0 830 180" role="img" aria-label="Where the plaza budget goes"><defs><marker id="tr-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path class="tr-head" d="M0 0L10 5L0 10z"/></marker></defs>'+s(0,55,"Budget",n)+s(220,55,"Escrow",e)+s(440,10,"Spend",t)+s(440,100,"Payment",i)+s(660,55,"Wallets",r)+o(170,90,220,90)+o(390,90,440,45)+o(390,90,440,135)+o(610,135,660,90)+"</svg>",l=x("div.tr-flow-wrap");return l.innerHTML=a,l};function qf(n,e){let t=x("div.stack"),i=!0,r,s="",o=null,a=()=>{let v=Jt({type:"number",min:1,step:1,required:!0}),f=()=>{},h=Ye("Mint",()=>{let M=Number(v.value);if(!Number.isInteger(M)||M<=0){rn("Enter a whole amount above zero.","error");return}h.disabled=!0,He.mintBudget(M).then(()=>{rn(`Minted ${xi(M)} into the plaza budget.`),f(),u()}).catch(P=>{h.disabled=!1,rn(ku(P),"error")})});h.classList.add("primary"),f=Ln("Mint budget",x("div.stack",null,yt("Amount",v,"Whole units added to the plaza budget."),x("div.row",null,h)))},l=()=>{let v=o?.roster??[];if(v.length===0){rn("There are no agents to fine.","error");return}let f=pn(v.map(T=>({value:String(T.cid),label:T.name}))),h=Jt({type:"number",min:1,step:1,required:!0}),M=Jt({type:"text",required:!0,placeholder:"What for"}),P=()=>{},w=Ye("Fine",()=>{let T=Number(h.value),E=M.value.trim();if(!Number.isInteger(T)||T<=0){rn("Enter a whole amount above zero.","error");return}if(!E){rn("A fine needs a memo.","error");return}w.disabled=!0,He.fineAgent(Number(f.value),T,E).then(()=>{rn(`Fined ${xi(T)}.`),P(),u()}).catch(U=>{w.disabled=!1,rn(ku(U),"error")})});w.classList.add("danger"),P=Ln("Fine an agent",x("div.stack",null,yt("Agent",f),yt("Amount",h,"Taken from the agent\u2019s wallet."),yt("Memo",M,"Shown in the journal."),x("div.row",null,w)))},c=Ye("Fine an agent",()=>l()),d=Ye("Mint budget",()=>a());d.classList.add("primary"),n.append(x("div.view-head",null,x("h1",null,"Treasury"),x("div.actions",null,c,d)),t);let m=()=>{if(!o)return;let{t:v,journal:f,wallets:h,roster:M}=o,P=new Map(M.map(k=>[k.cid,k.role_label])),w=v.accounts.filter(k=>k.kind==="escrow").reduce((k,D)=>k+D.balance,0),T=Jy(f,"payment"),E=h.reduce((k,D)=>k+D.balance,0),U=[...new Set(f.map(k=>k.kind))].sort(),p=f.filter(k=>!s||k.kind===s).slice(0,50),S=[{label:"Agent",cell:k=>x("a",{href:`#/agents/${k.cid}`},k.name)},{label:"Role",cell:k=>P.get(k.cid)??"\u2014"},{label:"Balance",num:!0,cell:k=>xi(k.balance)}],y=h.slice().sort((k,D)=>D.balance-k.balance),R=pn([{value:"",label:"All kinds"},...U.map(k=>({value:k,label:k}))],s);R.addEventListener("change",()=>{s=R.value,m()});let I=p.map(k=>x("div.tr-row",null,x("span.tr-n",null,`#${k.id}`),x("span.tr-at",null,qt(k.at)),lt(k.kind,Ky[k.kind]??"muted"),x("span.tr-memo",null,k.memo||`${k.kind}${k.task?` for ${k.task}`:""}`),x("span.tr-amt",null,xi(Xf(k)))));Be(t,x("div.grid.tr-tallies",null,Zn("Plaza budget",xi(v.plaza_budget)),Zn("In escrow",xi(w),"held for open tasks"),Zn("Spent",xi(v.spent),"to providers and compute"),Zn("Paid to agents",xi(T),"margins for verified work"),Zn("Agents\u2019 wallets",xi(E))),Gt("Flow",jy(v.plaza_budget,w,v.spent,T,E)),Gt("Wallets",h.length?or(S,y):pt("user","No wallets yet."),{flush:!0}),Gt("Journal",p.length?x("div.tr-ledger",null,...I):pt("clock","No entries."),{flush:!0,actions:[R]}))},u=async()=>{try{let[v,f,h,M]=await Promise.all([He.treasury(),He.treasuryJournal(),He.treasuryWallets(),He.agents()]);if(!i)return;o={t:v,journal:f.slice().sort((P,w)=>w.at-P.at),wallets:h,roster:M},m()}catch(v){i&&Be(t,pt("alert","Treasury unreachable",ku(v)))}},_=Mt("treasury",()=>{r!==void 0&&clearTimeout(r),r=setTimeout(()=>{r=void 0,u()},400)});return u(),()=>{i=!1,_(),r!==void 0&&clearTimeout(r)}}var Ul=[{key:"overview",label:"Overview",icon:"overview",section:"Plaza",mount:Of},{key:"agents",label:"Agents",icon:"agents",section:"Plaza",mount:Zu},{key:"tasks",label:"Tasks",icon:"tasks",section:"Plaza",mount:Wf},{key:"floor",label:"Floor",icon:"floor",section:"Plaza",mount:Sf},{key:"conversations",label:"Conversations",icon:"conversations",section:"Social",mount:ju},{key:"board",label:"Board",icon:"board",section:"Social",mount:Ju},{key:"council",label:"Council",icon:"council",section:"Social",mount:th},{key:"treasury",label:"Treasury",icon:"treasury",section:"Social",mount:qf},{key:"models",label:"Models",icon:"models",section:"Setup",mount:Ff},{key:"machine",label:"Machines",icon:"machine",section:"Setup",mount:Lf}];var $f=new Map,Yf=new Map;function Qy(){let n=x("nav.nav",{"aria-label":"Screens"}),e="";Ul.forEach((r,s)=>{r.section!==e&&(e=r.section,n.appendChild(x("div.nav-section",null,e)));let o=x("span.count");Yf.set(r.key,o);let a=x("a",{href:bn(r.key)},x("span.k",null,String(s+1)),r.label,x("span.dots"),o);$f.set(r.key,a),n.appendChild(a)});let t=x("div.sidebar-foot");zu(r=>Be(t,x("span.row",null,li(r?"ok":"danger"),r?"Live":"Reconnecting\u2026")));let i=x("div.brand",null,x("span.brand-mark"),"AgentPlaza",x("small",null,"the plaza register"));return x("aside.sidebar",null,i,n,t)}function e1(n){let e=x("div.health");return sr.watch(t=>{if(!t)return Be(e,x("span",null,li("danger"),"apid unreachable"));Be(e,x("span",null,li(t.hub?"ok":"danger"),"Hub"),x("span",null,li(t.noded?"ok":"danger"),"Node"),x("span",null,li(t.agents.working?"info":"muted"),`${t.agents.working} working \xB7 ${t.agents.idle} idle`))}),x("header.topbar",null,n,x("span.spacer"),e)}function t1(){let n=x("div",{hidden:!0});return sr.watch(e=>{n.hidden=!e?.dev_models,Be(n,ai("alert"),"Development models are enabled: the model policy is relaxed (mock and local models allowed)."),n.className="banner danger"}),n}function n1(){Hu();let n=x("span.title"),e=x("main.view-host"),t=x("div.app",null,Qy(),x("div.main",null,e1(n),t1(),e));document.body.replaceChildren(t),ft.watch(s=>{let o=s.filter(a=>a.status==="working").length;Yf.get("agents").textContent=s.length?o?`${o}/${s.length}`:String(s.length):""});let i,r="";Uu(s=>{let o=Ul.find(c=>c.key===s.screen)??Ul[0];for(let[c,d]of $f)c===o.key?d.setAttribute("aria-current","page"):d.removeAttribute("aria-current");let a=`${o.key}/${s.params.join("/")}`;if(a===r)return;r=a,i?.(),n.textContent=o.label,document.title=`${o.label} \xB7 AgentPlaza`;let l=x("div.view");e.replaceChildren(l),i=o.mount(l,s.params)})}n1();
/*! Bundled license information:

three/build/three.core.js:
three/build/three.module.js:
  (**
   * @license
   * Copyright 2010-2026 Three.js Authors
   * SPDX-License-Identifier: MIT
   *)
*/
