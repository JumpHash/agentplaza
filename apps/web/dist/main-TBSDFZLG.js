function zu(n){let e=n.replace(/^#\/?/,"").split("/").filter(Boolean).map(decodeURIComponent);return{screen:e[0]??"overview",params:e.slice(1)}}function bn(n,...e){return`#/${[n,...e.map(t=>encodeURIComponent(String(t)))].join("/")}`}function Mn(n,...e){location.hash=bn(n,...e)}function Gu(n){window.addEventListener("hashchange",()=>n(zu(location.hash))),n(zu(location.hash))}var Ts=class extends Error{constructor(t,i){super(i);this.status=t}status};async function As(n,e,t){let i=await fetch(e,{method:n,headers:t===void 0?{}:{"content-type":"application/json"},body:t===void 0?void 0:JSON.stringify(t)}),r=await i.text(),s=r?JSON.parse(r):null;if(!i.ok){let o=s?.error??`${n} ${e}: ${i.status}`;throw new Ts(i.status,o)}return s}var Nt=n=>As("GET",n),zn=(n,e={})=>As("POST",n,e),ql=(n,e)=>As("PUT",n,e),Vu=(n,e)=>As("PATCH",n,e),Po=n=>As("DELETE",n),ze={overview:()=>Nt("/v1/overview"),providerKinds:()=>Nt("/v1/provider-kinds"),providers:()=>Nt("/v1/providers"),addProvider:n=>zn("/v1/providers",n),updateProvider:n=>ql(`/v1/providers/${n.id}`,n),removeProvider:n=>Po(`/v1/providers/${n}`),setSecret:(n,e)=>ql(`/v1/providers/${n}/secret`,{value:e}),testProvider:(n,e)=>zn(`/v1/providers/${n}/test`,e?{model:e}:{}),providerModels:n=>Nt(`/v1/providers/${n}/models`),models:()=>Nt("/v1/models"),settings:()=>Nt("/v1/settings"),saveSettings:n=>ql("/v1/settings",n),agents:()=>Nt("/v1/agents"),agent:n=>Nt(`/v1/agents/${n}`),spawnAgents:(n,e,t)=>zn("/v1/agents",{role:n,count:e,model:t}),setAgentModel:(n,e)=>Vu(`/v1/agents/${n}`,{model:e}),renameAgent:(n,e)=>Vu(`/v1/agents/${n}`,{name:e}),killAgent:n=>Po(`/v1/agents/${n}`),chat:(n,e)=>zn(`/v1/agents/${n}/chat`,{text:e}),assign:(n,e)=>zn(`/v1/agents/${n}/assign`,{task_id:e}),cancel:n=>zn(`/v1/agents/${n}/cancel`),stop:n=>zn(`/v1/agents/${n}/stop`),transcript:(n,e=0,t=1e3)=>Nt(`/v1/agents/${n}/transcript?after=${e}&limit=${t}`),tasks:()=>Nt("/v1/tasks").then(n=>n.tasks),task:n=>Nt(`/v1/tasks/${n}`),createTask:n=>zn("/v1/tasks",n),withdrawTask:n=>Po(`/v1/tasks/${n}`),board:()=>Nt("/v1/board").then(n=>n.posts),conversations:()=>Nt("/v1/conversations").then(n=>n.conversations),motions:()=>Nt("/v1/motions").then(n=>n.motions),decide:(n,e,t)=>zn(`/v1/motions/${n}/decide`,{decision:e,reason:t}),treasury:()=>Nt("/v1/treasury"),treasuryJournal:()=>Nt("/v1/treasury/journal").then(n=>Array.isArray(n)?n:n.entries),treasuryWallets:()=>Nt("/v1/treasury/wallets"),mintBudget:n=>zn("/v1/treasury/mint",{amount:n}),fineAgent:(n,e,t)=>zn("/v1/treasury/fine",{cid:n,amount:e,memo:t}),agora:()=>Nt("/v1/agora").then(n=>n.calls),profile:n=>Nt(`/v1/agents/${n}/profile`),society:()=>Nt("/v1/society"),rateAgent:(n,e)=>zn(`/v1/agents/${n}/rating`,{value:e}),memory:n=>Nt(`/v1/agents/${n}/memory`).then(e=>e.notes),memoryNote:(n,e)=>Nt(`/v1/agents/${n}/memory/${encodeURIComponent(e)}`),machine:()=>Nt("/v1/machine"),machines:()=>Nt("/v1/machines"),runs:()=>Nt("/v1/runs").then(n=>n.runs),run:n=>Nt(`/v1/runs/${n}`),createRun:(n,e)=>zn("/v1/runs",{n,vcpus:1,memory_mib:256,role:e}),killRun:n=>Po(`/v1/runs/${n}`)};var Yl=new Map,Rs=null,Zl=!1,Kl=new Set;function Mt(n,e){let t=Yl.get(n);return t||Yl.set(n,t=new Set),t.add(e),()=>t.delete(e)}function qu(n){return Kl.add(n),n(Zl),()=>Kl.delete(n)}var pp=["agent","work","transcript","delta","task","treasury","agora","board","conversation","motion","provider","run","vm","serial"];function mp(){if(!Rs){Rs=new EventSource("/v1/events"),Rs.onopen=()=>Wu(!0),Rs.onerror=()=>Wu(!1);for(let n of pp)Rs.addEventListener(n,e=>{let t;try{t=JSON.parse(e.data)}catch{return}Yl.get(n)?.forEach(i=>i(t))})}}function Wu(n){n!==Zl&&(Zl=n,Kl.forEach(e=>e(n)),n&&lr())}var Io=class{constructor(e){this.v=e}v;fns=new Set;get(){return this.v}set(e){this.v=e,this.fns.forEach(t=>t(e))}watch(e){return this.fns.add(e),e(this.v),()=>this.fns.delete(e)}},dt=new Io([]),cr=new Io(null),$l=null;function lr(){return $l??=ze.agents().then(n=>dt.set(n.sort((e,t)=>e.cid-t.cid))).catch(()=>{}).finally(()=>$l=null),$l}function Xu(){return ze.overview().then(n=>cr.set(n)).catch(()=>cr.set(null))}function gp(n){let e=dt.get().slice(),t=e.findIndex(i=>i.cid===n.cid);return t>=0?e[t]={...e[t],...n}:e.push(n),e.sort((i,r)=>i.cid-r.cid)}function $u(){mp(),lr(),Xu(),Mt("agent",n=>{dt.set(gp(n)),lr()}),Mt("work",()=>{lr()}),setInterval(()=>{Xu()},1e4),setInterval(()=>{lr()},15e3)}var Yu={anthropic:{title:"Anthropic",hex:"191919",path:"M17.3041 3.541h-3.6718l6.696 16.918H24Zm-10.6082 0L0 20.459h3.7442l1.3693-3.5527h7.0052l1.3693 3.5528h3.7442L10.5363 3.5409Zm-.3712 10.2232 2.2914-5.9456 2.2914 5.9456Z"},openai:{title:"OpenAI",hex:"412991",path:"M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z"},google:{title:"Google Gemini",hex:"8E75B2",path:"M11.04 19.32Q12 21.51 12 24q0-2.49.93-4.68.96-2.19 2.58-3.81t3.81-2.55Q21.51 12 24 12q-2.49 0-4.68-.93a12.3 12.3 0 0 1-3.81-2.58 12.3 12.3 0 0 1-2.58-3.81Q12 2.49 12 0q0 2.49-.96 4.68-.93 2.19-2.55 3.81a12.3 12.3 0 0 1-3.81 2.58Q2.49 12 0 12q2.49 0 4.68.96 2.19.93 3.81 2.55t2.55 3.81"},aws:{title:"Amazon AWS",hex:"232F3E",path:"M6.763 10.036c0 .296.032.535.088.71.064.176.144.368.256.576.04.063.056.127.056.183 0 .08-.048.16-.152.24l-.503.335a.383.383 0 0 1-.208.072c-.08 0-.16-.04-.239-.112a2.47 2.47 0 0 1-.287-.375 6.18 6.18 0 0 1-.248-.471c-.622.734-1.405 1.101-2.347 1.101-.67 0-1.205-.191-1.596-.574-.391-.384-.59-.894-.59-1.533 0-.678.239-1.23.726-1.644.487-.415 1.133-.623 1.955-.623.272 0 .551.024.846.064.296.04.6.104.918.176v-.583c0-.607-.127-1.03-.375-1.277-.255-.248-.686-.367-1.3-.367-.28 0-.568.031-.863.103-.295.072-.583.16-.862.272a2.287 2.287 0 0 1-.28.104.488.488 0 0 1-.127.023c-.112 0-.168-.08-.168-.247v-.391c0-.128.016-.224.056-.28a.597.597 0 0 1 .224-.167c.279-.144.614-.264 1.005-.36a4.84 4.84 0 0 1 1.246-.151c.95 0 1.644.216 2.091.647.439.43.662 1.085.662 1.963v2.586zm-3.24 1.214c.263 0 .534-.048.822-.144.287-.096.543-.271.758-.51.128-.152.224-.32.272-.512.047-.191.08-.423.08-.694v-.335a6.66 6.66 0 0 0-.735-.136 6.02 6.02 0 0 0-.75-.048c-.535 0-.926.104-1.19.32-.263.215-.39.518-.39.917 0 .375.095.655.295.846.191.2.47.296.838.296zm6.41.862c-.144 0-.24-.024-.304-.08-.064-.048-.12-.16-.168-.311L7.586 5.55a1.398 1.398 0 0 1-.072-.32c0-.128.064-.2.191-.2h.783c.151 0 .255.025.31.08.065.048.113.16.16.312l1.342 5.284 1.245-5.284c.04-.16.088-.264.151-.312a.549.549 0 0 1 .32-.08h.638c.152 0 .256.025.32.08.063.048.12.16.151.312l1.261 5.348 1.381-5.348c.048-.16.104-.264.16-.312a.52.52 0 0 1 .311-.08h.743c.127 0 .2.065.2.2 0 .04-.009.08-.017.128a1.137 1.137 0 0 1-.056.2l-1.923 6.17c-.048.16-.104.263-.168.311a.51.51 0 0 1-.303.08h-.687c-.151 0-.255-.024-.32-.08-.063-.056-.119-.16-.15-.32l-1.238-5.148-1.23 5.14c-.04.16-.087.264-.15.32-.065.056-.177.08-.32.08zm10.256.215c-.415 0-.83-.048-1.229-.143-.399-.096-.71-.2-.918-.32-.128-.071-.215-.151-.247-.223a.563.563 0 0 1-.048-.224v-.407c0-.167.064-.247.183-.247.048 0 .096.008.144.024.048.016.12.048.2.08.271.12.566.215.878.279.319.064.63.096.95.096.502 0 .894-.088 1.165-.264a.86.86 0 0 0 .415-.758.777.777 0 0 0-.215-.559c-.144-.151-.416-.287-.807-.415l-1.157-.36c-.583-.183-1.014-.454-1.277-.813a1.902 1.902 0 0 1-.4-1.158c0-.335.073-.63.216-.886.144-.255.335-.479.575-.654.24-.184.51-.32.83-.415.32-.096.655-.136 1.006-.136.175 0 .359.008.535.032.183.024.35.056.518.088.16.04.312.08.455.127.144.048.256.096.336.144a.69.69 0 0 1 .24.2.43.43 0 0 1 .071.263v.375c0 .168-.064.256-.184.256a.83.83 0 0 1-.303-.096 3.652 3.652 0 0 0-1.532-.311c-.455 0-.815.071-1.062.223-.248.152-.375.383-.375.71 0 .224.08.416.24.567.159.152.454.304.877.44l1.134.358c.574.184.99.44 1.237.767.247.327.367.702.367 1.117 0 .343-.072.655-.207.926-.144.272-.336.511-.583.703-.248.2-.543.343-.886.447-.36.111-.734.167-1.142.167zM21.698 16.207c-2.626 1.94-6.442 2.969-9.722 2.969-4.598 0-8.74-1.7-11.87-4.526-.247-.223-.024-.527.272-.351 3.384 1.963 7.559 3.153 11.877 3.153 2.914 0 6.114-.607 9.06-1.852.439-.2.814.287.383.607zM22.792 14.961c-.336-.43-2.22-.207-3.074-.103-.255.032-.295-.192-.063-.36 1.5-1.053 3.967-.75 4.254-.399.287.36-.08 2.826-1.485 4.007-.215.184-.423.088-.327-.151.32-.79 1.03-2.57.695-2.994z"},azure:{title:"Microsoft Azure",hex:"0078D4",path:"M22.379 23.343a1.62 1.62 0 0 0 1.536-2.14v.002L17.35 1.76A1.62 1.62 0 0 0 15.816.657H8.184A1.62 1.62 0 0 0 6.65 1.76L.086 21.204a1.62 1.62 0 0 0 1.536 2.139h4.741a1.62 1.62 0 0 0 1.535-1.103l.977-2.892 4.947 3.675c.28.208.618.32.966.32m-3.084-12.531 3.624 10.739a.54.54 0 0 1-.51.713v-.001h-.03a.54.54 0 0 1-.322-.106l-9.287-6.9h4.853m6.313 7.006c.116-.326.13-.694.007-1.058L9.79 1.76a1.722 1.722 0 0 0-.007-.02h6.034a.54.54 0 0 1 .512.366l6.562 19.445a.54.54 0 0 1-.338.684"},xai:{title:"X",hex:"000000",path:"M14.234 10.162 22.977 0h-2.072l-7.591 8.824L7.251 0H.258l9.168 13.343L.258 24H2.33l8.016-9.318L16.749 24h6.993zm-2.837 3.299-.929-1.329L3.076 1.56h3.182l5.965 8.532.929 1.329 7.754 11.09h-3.182z"},deepseek:{title:"DeepSeek",hex:"5786FE",path:"M23.748 4.651c-.254-.124-.364.113-.512.233-.051.04-.094.09-.137.137-.372.397-.806.657-1.373.626-.829-.046-1.537.214-2.163.848-.133-.782-.575-1.248-1.247-1.548-.352-.155-.708-.311-.955-.65-.172-.24-.219-.509-.305-.774-.055-.16-.11-.323-.293-.35-.2-.031-.278.136-.356.276-.313.572-.434 1.202-.422 1.84.027 1.436.633 2.58 1.838 3.393.137.094.172.187.129.323-.082.28-.18.553-.266.833-.055.179-.137.218-.328.14a5.5 5.5 0 0 1-1.737-1.179c-.857-.828-1.631-1.743-2.597-2.46a12 12 0 0 0-.689-.47c-.985-.957.13-1.743.387-1.836.27-.098.094-.433-.778-.428-.872.003-1.67.295-2.687.685a3 3 0 0 1-.465.136 9.6 9.6 0 0 0-2.883-.101c-1.885.21-3.39 1.1-4.497 2.622C.082 8.776-.231 10.854.152 13.02c.403 2.284 1.568 4.175 3.36 5.653 1.857 1.533 3.997 2.284 6.438 2.14 1.482-.085 3.132-.284 4.994-1.86.47.234.962.328 1.78.398.629.058 1.235-.031 1.705-.129.735-.155.684-.836.418-.961-2.155-1.004-1.682-.595-2.112-.926 1.095-1.295 2.768-3.598 3.284-6.733.05-.346.115-.834.108-1.114-.004-.171.035-.238.23-.257a4.2 4.2 0 0 0 1.545-.475c1.397-.763 1.96-2.016 2.093-3.517.02-.23-.004-.467-.247-.588M11.58 18.168c-2.088-1.642-3.101-2.183-3.52-2.16-.39.024-.32.472-.234.763.09.288.207.487.371.74.114.167.192.416-.113.603-.673.416-1.842-.14-1.897-.168-1.361-.801-2.5-1.86-3.301-3.306-.775-1.393-1.225-2.888-1.299-4.482-.02-.385.094-.522.477-.592a4.7 4.7 0 0 1 1.53-.038c2.131.311 3.946 1.264 5.467 2.774.868.86 1.525 1.887 2.202 2.89.72 1.066 1.494 2.082 2.48 2.915.348.291.626.513.892.677-.802.09-2.14.109-3.055-.615zm1.001-6.44a.306.306 0 0 1 .415-.287.3.3 0 0 1 .113.074.3.3 0 0 1 .086.214c0 .17-.136.307-.308.307a.303.303 0 0 1-.306-.307m3.11 1.596c-.2.081-.4.151-.591.16a1.25 1.25 0 0 1-.798-.254c-.274-.23-.47-.358-.551-.758a1.7 1.7 0 0 1 .015-.588c.07-.327-.007-.537-.238-.727-.188-.156-.426-.199-.689-.199a.6.6 0 0 1-.254-.078.253.253 0 0 1-.114-.358 1 1 0 0 1 .192-.21c.356-.202.767-.136 1.146.016.352.144.618.408 1.001.782.392.451.462.576.685.915.176.264.336.536.446.848.066.194-.02.353-.25.45"},moonshot:{title:"Moonshot AI",hex:"000000",path:"m1.053 16.91 9.538 2.55a21 20.981 0 0 0 .06 2.031l5.956 1.592a12 11.99 0 0 1-15.554-6.172m-1.02-5.79 11.352 3.035a21 20.981 0 0 0-.469 2.01l10.817 2.89a12 11.99 0 0 1-1.845 2.004L.658 15.918a12 11.99 0 0 1-.625-4.796m1.593-5.146L13.573 9.17a21 20.981 0 0 0-1.01 1.874l11.297 3.02a21 20.981 0 0 1-.67 2.362l-11.55-3.087L.125 10.26a12 11.99 0 0 1 1.499-4.285ZM6.067 1.58l11.285 3.016a21 20.981 0 0 0-1.688 1.719l7.824 2.091a21 20.981 0 0 1 .513 2.664L2.107 5.218a12 11.99 0 0 1 3.96-3.638M21.68 4.866 7.222 1.003A12 11.99 0 0 1 21.68 4.866"},mistral:{title:"Mistral AI",hex:"FA520F",path:"M17.143 3.429v3.428h-3.429v3.429h-3.428V6.857H6.857V3.43H3.43v13.714H0v3.428h10.286v-3.428H6.857v-3.429h3.429v3.429h3.429v-3.429h3.428v3.429h-3.428v3.428H24v-3.428h-3.43V3.429z"},openrouter:{title:"OpenRouter",hex:"94A3B8",path:"M16.778 1.844v1.919q-.569-.026-1.138-.032-.708-.008-1.415.037c-1.93.126-4.023.728-6.149 2.237-2.911 2.066-2.731 1.95-4.14 2.75-.396.223-1.342.574-2.185.798-.841.225-1.753.333-1.751.333v4.229s.768.108 1.61.333c.842.224 1.789.575 2.185.799 1.41.798 1.228.683 4.14 2.75 2.126 1.509 4.22 2.11 6.148 2.236.88.058 1.716.041 2.555.005v1.918l7.222-4.168-7.222-4.17v2.176c-.86.038-1.611.065-2.278.021-1.364-.09-2.417-.357-3.979-1.465-2.244-1.593-2.866-2.027-3.68-2.508.889-.518 1.449-.906 3.822-2.59 1.56-1.109 2.614-1.377 3.978-1.466.667-.044 1.418-.017 2.278.02v2.176L24 6.014Z"},perplexity:{title:"Perplexity",hex:"1FB8CD",path:"M22.3977 7.0896h-2.3106V.0676l-7.5094 6.3542V.1577h-1.1554v6.1966L4.4904 0v7.0896H1.6023v10.3976h2.8882V24l6.932-6.3591v6.2005h1.1554v-6.0469l6.9318 6.1807v-6.4879h2.8882V7.0896zm-3.4657-4.531v4.531h-5.355l5.355-4.531zm-13.2862.0676 4.8691 4.4634H5.6458V2.6262zM2.7576 16.332V8.245h7.8476l-6.1149 6.1147v1.9723H2.7576zm2.8882 5.0404v-3.8852h.0001v-2.6488l5.7763-5.7764v7.0111l-5.7764 5.2993zm12.7086.0248-5.7766-5.1509V9.0618l5.7766 5.7766v6.5588zm2.8882-5.0652h-1.733v-1.9723L13.3948 8.245h7.8478v8.087z"},nvidia:{title:"NVIDIA",hex:"76B900",path:"M8.948 8.798v-1.43a6.7 6.7 0 0 1 .424-.018c3.922-.124 6.493 3.374 6.493 3.374s-2.774 3.851-5.75 3.851c-.398 0-.787-.062-1.158-.185v-4.346c1.528.185 1.837.857 2.747 2.385l2.04-1.714s-1.492-1.952-4-1.952a6.016 6.016 0 0 0-.796.035m0-4.735v2.138l.424-.027c5.45-.185 9.01 4.47 9.01 4.47s-4.08 4.964-8.33 4.964c-.37 0-.733-.035-1.095-.097v1.325c.3.035.61.062.91.062 3.957 0 6.82-2.023 9.593-4.408.459.371 2.34 1.263 2.73 1.652-2.633 2.208-8.772 3.984-12.253 3.984-.335 0-.653-.018-.971-.053v1.864H24V4.063zm0 10.326v1.131c-3.657-.654-4.673-4.46-4.673-4.46s1.758-1.944 4.673-2.262v1.237H8.94c-1.528-.186-2.73 1.245-2.73 1.245s.68 2.412 2.739 3.11M2.456 10.9s2.164-3.197 6.5-3.533V6.201C4.153 6.59 0 10.653 0 10.653s2.35 6.802 8.948 7.42v-1.237c-4.84-.6-6.492-5.936-6.492-5.936z"},ollama:{title:"Ollama",hex:"000000",path:"M16.361 10.26a.894.894 0 0 0-.558.47l-.072.148.001.207c0 .193.004.217.059.353.076.193.152.312.291.448.24.238.51.3.872.205a.86.86 0 0 0 .517-.436.752.752 0 0 0 .08-.498c-.064-.453-.33-.782-.724-.897a1.06 1.06 0 0 0-.466 0zm-9.203.005c-.305.096-.533.32-.65.639a1.187 1.187 0 0 0-.06.52c.057.309.31.59.598.667.362.095.632.033.872-.205.14-.136.215-.255.291-.448.055-.136.059-.16.059-.353l.001-.207-.072-.148a.894.894 0 0 0-.565-.472 1.02 1.02 0 0 0-.474.007Zm4.184 2c-.131.071-.223.25-.195.383.031.143.157.288.353.407.105.063.112.072.117.136.004.038-.01.146-.029.243-.02.094-.036.194-.036.222.002.074.07.195.143.253.064.052.076.054.255.059.164.005.198.001.264-.03.169-.082.212-.234.15-.525-.052-.243-.042-.28.087-.355.137-.08.281-.219.324-.314a.365.365 0 0 0-.175-.48.394.394 0 0 0-.181-.033c-.126 0-.207.03-.355.124l-.085.053-.053-.032c-.219-.13-.259-.145-.391-.143a.396.396 0 0 0-.193.032zm.39-2.195c-.373.036-.475.05-.654.086-.291.06-.68.195-.951.328-.94.46-1.589 1.226-1.787 2.114-.04.176-.045.234-.045.53 0 .294.005.357.043.524.264 1.16 1.332 2.017 2.714 2.173.3.033 1.596.033 1.896 0 1.11-.125 2.064-.727 2.493-1.571.114-.226.169-.372.22-.602.039-.167.044-.23.044-.523 0-.297-.005-.355-.045-.531-.288-1.29-1.539-2.304-3.072-2.497a6.873 6.873 0 0 0-.855-.031zm.645.937a3.283 3.283 0 0 1 1.44.514c.223.148.537.458.671.662.166.251.26.508.303.82.02.143.01.251-.043.482-.08.345-.332.705-.672.957a3.115 3.115 0 0 1-.689.348c-.382.122-.632.144-1.525.138-.582-.006-.686-.01-.853-.042-.57-.107-1.022-.334-1.35-.68-.264-.28-.385-.535-.45-.946-.03-.192.025-.509.137-.776.136-.326.488-.73.836-.963.403-.269.934-.46 1.422-.512.187-.02.586-.02.773-.002zm-5.503-11a1.653 1.653 0 0 0-.683.298C5.617.74 5.173 1.666 4.985 2.819c-.07.436-.119 1.04-.119 1.503 0 .544.064 1.24.155 1.721.02.107.031.202.023.208a8.12 8.12 0 0 1-.187.152 5.324 5.324 0 0 0-.949 1.02 5.49 5.49 0 0 0-.94 2.339 6.625 6.625 0 0 0-.023 1.357c.091.78.325 1.438.727 2.04l.13.195-.037.064c-.269.452-.498 1.105-.605 1.732-.084.496-.095.629-.095 1.294 0 .67.009.803.088 1.266.095.555.288 1.143.503 1.534.071.128.243.393.264.407.007.003-.014.067-.046.141a7.405 7.405 0 0 0-.548 1.873c-.062.417-.071.552-.071.991 0 .56.031.832.148 1.279L3.42 24h1.478l-.05-.091c-.297-.552-.325-1.575-.068-2.597.117-.472.25-.819.498-1.296l.148-.29v-.177c0-.165-.003-.184-.057-.293a.915.915 0 0 0-.194-.25 1.74 1.74 0 0 1-.385-.543c-.424-.92-.506-2.286-.208-3.451.124-.486.329-.918.544-1.154a.787.787 0 0 0 .223-.531c0-.195-.07-.355-.224-.522a3.136 3.136 0 0 1-.817-1.729c-.14-.96.114-2.005.69-2.834.563-.814 1.353-1.336 2.237-1.475.199-.033.57-.028.776.01.226.04.367.028.512-.041.179-.085.268-.19.374-.431.093-.215.165-.333.36-.576.234-.29.46-.489.822-.729.413-.27.884-.467 1.352-.561.17-.035.25-.04.569-.04.319 0 .398.005.569.04a4.07 4.07 0 0 1 1.914.997c.117.109.398.457.488.602.034.057.095.177.132.267.105.241.195.346.374.43.14.068.286.082.503.045.343-.058.607-.053.943.016 1.144.23 2.14 1.173 2.581 2.437.385 1.108.276 2.267-.296 3.153-.097.15-.193.27-.333.419-.301.322-.301.722-.001 1.053.493.539.801 1.866.708 3.036-.062.772-.26 1.463-.533 1.854a2.096 2.096 0 0 1-.224.258.916.916 0 0 0-.194.25c-.054.109-.057.128-.057.293v.178l.148.29c.248.476.38.823.498 1.295.253 1.008.231 2.01-.059 2.581a.845.845 0 0 0-.044.098c0 .006.329.009.732.009h.73l.02-.074.036-.134c.019-.076.057-.3.088-.516.029-.217.029-1.016 0-1.258-.11-.875-.295-1.57-.597-2.226-.032-.074-.053-.138-.046-.141.008-.005.057-.074.108-.152.376-.569.607-1.284.724-2.228.031-.26.031-1.378 0-1.628-.083-.645-.182-1.082-.348-1.525a6.083 6.083 0 0 0-.329-.7l-.038-.064.131-.194c.402-.604.636-1.262.727-2.04a6.625 6.625 0 0 0-.024-1.358 5.512 5.512 0 0 0-.939-2.339 5.325 5.325 0 0 0-.95-1.02 8.097 8.097 0 0 1-.186-.152.692.692 0 0 1 .023-.208c.208-1.087.201-2.443-.017-3.503-.19-.924-.535-1.658-.98-2.082-.354-.338-.716-.482-1.15-.455-.996.059-1.8 1.205-2.116 3.01a6.805 6.805 0 0 0-.097.726c0 .036-.007.066-.015.066a.96.96 0 0 1-.149-.078A4.857 4.857 0 0 0 12 3.03c-.832 0-1.687.243-2.456.698a.958.958 0 0 1-.148.078c-.008 0-.015-.03-.015-.066a6.71 6.71 0 0 0-.097-.725C8.997 1.392 8.337.319 7.46.048a2.096 2.096 0 0 0-.585-.041Zm.293 1.402c.248.197.523.759.682 1.388.03.113.06.244.069.292.007.047.026.152.041.233.067.365.098.76.102 1.24l.002.475-.12.175-.118.178h-.278c-.324 0-.646.041-.954.124l-.238.06c-.033.007-.038-.003-.057-.144a8.438 8.438 0 0 1 .016-2.323c.124-.788.413-1.501.696-1.711.067-.05.079-.049.157.013zm9.825-.012c.17.126.358.46.498.888.28.854.36 2.028.212 3.145-.019.14-.024.151-.057.144l-.238-.06a3.693 3.693 0 0 0-.954-.124h-.278l-.119-.178-.119-.175.002-.474c.004-.669.066-1.19.214-1.772.157-.623.434-1.185.68-1.382.078-.062.09-.063.159-.012z"},lmstudio:{title:"LM Studio",hex:"000000",path:"M14.025 0c3.492 0 5.237 0 6.571.68a6.24 6.24 0 0 1 2.725 2.724C24 4.738 24 6.484 24 9.975v4.05c0 3.492 0 5.237-.68 6.571a6.24 6.24 0 0 1-2.724 2.725c-1.334.679-3.08.679-6.571.679h-4.05c-3.492 0-5.237 0-6.571-.68A6.24 6.24 0 0 1 .68 20.597C0 19.262 0 17.516 0 14.025v-4.05c0-3.492 0-5.237.68-6.571A6.23 6.23 0 0 1 3.404.68C4.738 0 6.484 0 9.975 0zM7.688 16.313a1.313 1.313 0 0 0 0 2.625h11.625a1.313 1.313 0 0 0 0-2.625zm-3-3.75a1.313 1.313 0 0 0 0 2.624h11.625a1.313 1.313 0 0 0 0-2.624zm3-3.75a1.313 1.313 0 0 0 0 2.624h11.625a1.313 1.313 0 0 0 0-2.624zm-3-3.75a1.313 1.313 0 0 0 0 2.625h11.625a1.313 1.313 0 0 0 0-2.625z"},vllm:{title:"vLLM",hex:"30A2FF",path:"m23.6 0-8.721 4.59L9.829 24h7.41zM9.83 24V5.142H.4Z"},huggingface:{title:"Hugging Face",hex:"FFD21E",path:"M12.025 1.13c-5.77 0-10.449 4.647-10.449 10.378 0 1.112.178 2.181.503 3.185.064-.222.203-.444.416-.577a.96.96 0 0 1 .524-.15c.293 0 .584.124.84.284.278.173.48.408.71.694.226.282.458.611.684.951v-.014c.017-.324.106-.622.264-.874s.403-.487.762-.543c.3-.047.596.06.787.203s.31.313.4.467c.15.257.212.468.233.542.01.026.653 1.552 1.657 2.54.616.605 1.01 1.223 1.082 1.912.055.537-.096 1.059-.38 1.572.637.121 1.294.187 1.967.187.657 0 1.298-.063 1.921-.178-.287-.517-.44-1.041-.384-1.581.07-.69.465-1.307 1.081-1.913 1.004-.987 1.647-2.513 1.657-2.539.021-.074.083-.285.233-.542.09-.154.208-.323.4-.467a1.08 1.08 0 0 1 .787-.203c.359.056.604.29.762.543s.247.55.265.874v.015c.225-.34.457-.67.683-.952.23-.286.432-.52.71-.694.257-.16.547-.284.84-.285a.97.97 0 0 1 .524.151c.228.143.373.388.43.625l.006.04a10.3 10.3 0 0 0 .534-3.273c0-5.731-4.678-10.378-10.449-10.378M8.327 6.583a1.5 1.5 0 0 1 .713.174 1.487 1.487 0 0 1 .617 2.013c-.183.343-.762-.214-1.102-.094-.38.134-.532.914-.917.71a1.487 1.487 0 0 1 .69-2.803m7.486 0a1.487 1.487 0 0 1 .689 2.803c-.385.204-.536-.576-.916-.71-.34-.12-.92.437-1.103.094a1.487 1.487 0 0 1 .617-2.013 1.5 1.5 0 0 1 .713-.174m-10.68 1.55a.96.96 0 1 1 0 1.921.96.96 0 0 1 0-1.92m13.838 0a.96.96 0 1 1 0 1.92.96.96 0 0 1 0-1.92M8.489 11.458c.588.01 1.965 1.157 3.572 1.164 1.607-.007 2.984-1.155 3.572-1.164.196-.003.305.12.305.454 0 .886-.424 2.328-1.563 3.202-.22-.756-1.396-1.366-1.63-1.32q-.011.001-.02.006l-.044.026-.01.008-.03.024q-.018.017-.035.036l-.032.04a1 1 0 0 0-.058.09l-.014.025q-.049.088-.11.19a1 1 0 0 1-.083.116 1.2 1.2 0 0 1-.173.18q-.035.029-.075.058a1.3 1.3 0 0 1-.251-.243 1 1 0 0 1-.076-.107c-.124-.193-.177-.363-.337-.444-.034-.016-.104-.008-.2.022q-.094.03-.216.087-.06.028-.125.063l-.13.074q-.067.04-.136.086a3 3 0 0 0-.135.096 3 3 0 0 0-.26.219 2 2 0 0 0-.12.121 2 2 0 0 0-.106.128l-.002.002a2 2 0 0 0-.09.132l-.001.001a1.2 1.2 0 0 0-.105.212q-.013.036-.024.073c-1.139-.875-1.563-2.317-1.563-3.203 0-.334.109-.457.305-.454m.836 10.354c.824-1.19.766-2.082-.365-3.194-1.13-1.112-1.789-2.738-1.789-2.738s-.246-.945-.806-.858-.97 1.499.202 2.362c1.173.864-.233 1.45-.685.64-.45-.812-1.683-2.896-2.322-3.295s-1.089-.175-.938.647 2.822 2.813 2.562 3.244-1.176-.506-1.176-.506-2.866-2.567-3.49-1.898.473 1.23 2.037 2.16c1.564.932 1.686 1.178 1.464 1.53s-3.675-2.511-4-1.297c-.323 1.214 3.524 1.567 3.287 2.405-.238.839-2.71-1.587-3.216-.642-.506.946 3.49 2.056 3.522 2.064 1.29.33 4.568 1.028 5.713-.624m5.349 0c-.824-1.19-.766-2.082.365-3.194 1.13-1.112 1.789-2.738 1.789-2.738s.246-.945.806-.858.97 1.499-.202 2.362c-1.173.864.233 1.45.685.64.451-.812 1.683-2.896 2.322-3.295s1.089-.175.938.647-2.822 2.813-2.562 3.244 1.176-.506 1.176-.506 2.866-2.567 3.49-1.898-.473 1.23-2.037 2.16c-1.564.932-1.686 1.178-1.464 1.53s3.675-2.511 4-1.297c.323 1.214-3.524 1.567-3.287 2.405.238.839 2.71-1.587 3.216-.642.506.946-3.49 2.056-3.522 2.064-1.29.33-4.568 1.028-5.713-.624"}};var Zu={overview:[["rect",{width:"7",height:"9",x:"3",y:"3",rx:"1"}],["rect",{width:"7",height:"5",x:"14",y:"3",rx:"1"}],["rect",{width:"7",height:"9",x:"14",y:"12",rx:"1"}],["rect",{width:"7",height:"5",x:"3",y:"16",rx:"1"}]],agents:[["path",{d:"M12 8V4H8"}],["rect",{width:"16",height:"12",x:"4",y:"8",rx:"2"}],["path",{d:"M2 14h2"}],["path",{d:"M20 14h2"}],["path",{d:"M15 13v2"}],["path",{d:"M9 13v2"}]],tasks:[["path",{d:"M13 5h8"}],["path",{d:"M13 12h8"}],["path",{d:"M13 19h8"}],["path",{d:"m3 17 2 2 4-4"}],["path",{d:"m3 7 2 2 4-4"}]],models:[["path",{d:"M12 20v2"}],["path",{d:"M12 2v2"}],["path",{d:"M17 20v2"}],["path",{d:"M17 2v2"}],["path",{d:"M2 12h2"}],["path",{d:"M2 17h2"}],["path",{d:"M2 7h2"}],["path",{d:"M20 12h2"}],["path",{d:"M20 17h2"}],["path",{d:"M20 7h2"}],["path",{d:"M7 20v2"}],["path",{d:"M7 2v2"}],["rect",{x:"4",y:"4",width:"16",height:"16",rx:"2"}],["rect",{x:"8",y:"8",width:"8",height:"8",rx:"1"}]],conversations:[["path",{d:"M16 10a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 14.286V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"}],["path",{d:"M20 9a2 2 0 0 1 2 2v10.286a.71.71 0 0 1-1.212.502l-2.202-2.202A2 2 0 0 0 17.172 19H10a2 2 0 0 1-2-2v-1"}]],board:[["path",{d:"M11 6a13 13 0 0 0 8.4-2.8A1 1 0 0 1 21 4v12a1 1 0 0 1-1.6.8A13 13 0 0 0 11 14H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2z"}],["path",{d:"M6 14a12 12 0 0 0 2.4 7.2 2 2 0 0 0 3.2-2.4A8 8 0 0 1 10 14"}],["path",{d:"M8 6v8"}]],council:[["path",{d:"M10 18v-7"}],["path",{d:"M11.119 2.205a2 2 0 0 1 1.762 0l7.84 3.846A.5.5 0 0 1 20.5 7h-17a.5.5 0 0 1-.22-.949z"}],["path",{d:"M14 18v-7"}],["path",{d:"M18 18v-7"}],["path",{d:"M3 22h18"}],["path",{d:"M6 18v-7"}]],treasury:[["path",{d:"M13.744 17.736a6 6 0 1 1-7.48-7.48"}],["path",{d:"M15 6h1v4"}],["path",{d:"m6.134 14.768.866-.5 2 3.464"}],["circle",{cx:"16",cy:"8",r:"6"}]],floor:[["path",{d:"M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"}],["path",{d:"m3.3 7 8.7 5 8.7-5"}],["path",{d:"M12 22V12"}]],machine:[["rect",{width:"20",height:"8",x:"2",y:"2",rx:"2",ry:"2"}],["rect",{width:"20",height:"8",x:"2",y:"14",rx:"2",ry:"2"}],["line",{x1:"6",x2:"6.01",y1:"6",y2:"6"}],["line",{x1:"6",x2:"6.01",y1:"18",y2:"18"}]],plus:[["path",{d:"M5 12h14"}],["path",{d:"M12 5v14"}]],send:[["path",{d:"M3.714 3.048a.498.498 0 0 0-.683.627l2.843 7.627a2 2 0 0 1 0 1.396l-2.842 7.627a.498.498 0 0 0 .682.627l18-8.5a.5.5 0 0 0 0-.904z"}],["path",{d:"M6 12h16"}]],stop:[["rect",{width:"18",height:"18",x:"3",y:"3",rx:"2"}]],x:[["path",{d:"M18 6 6 18"}],["path",{d:"m6 6 12 12"}]],check:[["path",{d:"M20 6 9 17l-5-5"}]],alert:[["path",{d:"m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"}],["path",{d:"M12 9v4"}],["path",{d:"M12 17h.01"}]],play:[["path",{d:"M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"}]],refresh:[["path",{d:"M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"}],["path",{d:"M21 3v5h-5"}],["path",{d:"M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"}],["path",{d:"M8 16H3v5"}]],key:[["path",{d:"M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z"}],["circle",{cx:"16.5",cy:"7.5",r:".5",fill:"currentColor"}]],trash:[["path",{d:"M10 11v6"}],["path",{d:"M14 11v6"}],["path",{d:"M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"}],["path",{d:"M3 6h18"}],["path",{d:"M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"}]],edit:[["path",{d:"M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"}],["path",{d:"m15 5 4 4"}]],chevronRight:[["path",{d:"m9 18 6-6-6-6"}]],chevronDown:[["path",{d:"m6 9 6 6 6-6"}]],terminal:[["path",{d:"M12 19h8"}],["path",{d:"m4 17 6-6-6-6"}]],wrench:[["path",{d:"M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.106-3.105c.32-.322.863-.22.983.218a6 6 0 0 1-8.259 7.057l-7.91 7.91a1 1 0 0 1-2.999-3l7.91-7.91a6 6 0 0 1 7.057-8.259c.438.12.54.662.219.984z"}]],brain:[["path",{d:"M12 18V5"}],["path",{d:"M15 13a4.17 4.17 0 0 1-3-4 4.17 4.17 0 0 1-3 4"}],["path",{d:"M17.598 6.5A3 3 0 1 0 12 5a3 3 0 1 0-5.598 1.5"}],["path",{d:"M17.997 5.125a4 4 0 0 1 2.526 5.77"}],["path",{d:"M18 18a4 4 0 0 0 2-7.464"}],["path",{d:"M19.967 17.483A4 4 0 1 1 12 18a4 4 0 1 1-7.967-.517"}],["path",{d:"M6 18a4 4 0 0 1-2-7.464"}],["path",{d:"M6.003 5.125a4 4 0 0 0-2.526 5.77"}]],user:[["path",{d:"M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"}],["circle",{cx:"12",cy:"7",r:"4"}]],society:[["circle",{cx:"18",cy:"5",r:"3"}],["circle",{cx:"6",cy:"12",r:"3"}],["circle",{cx:"18",cy:"19",r:"3"}],["line",{x1:"8.59",x2:"15.42",y1:"13.51",y2:"17.49"}],["line",{x1:"15.41",x2:"8.59",y1:"6.51",y2:"10.49"}]],memory:[["path",{d:"M12 18V5"}],["path",{d:"M15 13a4.17 4.17 0 0 1-3-4 4.17 4.17 0 0 1-3 4"}],["path",{d:"M17.598 6.5A3 3 0 1 0 12 5a3 3 0 1 0-5.598 1.5"}],["path",{d:"M17.997 5.125a4 4 0 0 1 2.526 5.77"}],["path",{d:"M18 18a4 4 0 0 0 2-7.464"}],["path",{d:"M19.967 17.483A4 4 0 1 1 12 18a4 4 0 1 1-7.967-.517"}],["path",{d:"M6 18a4 4 0 0 1-2-7.464"}],["path",{d:"M6.003 5.125a4 4 0 0 0-2.526 5.77"}]],heart:[["path",{d:"M2 9.5a5.5 5.5 0 0 1 9.591-3.676.56.56 0 0 0 .818 0A5.49 5.49 0 0 1 22 9.5c0 2.29-1.5 4-3 5.5l-5.492 5.313a2 2 0 0 1-3 .019L5 15c-1.5-1.5-3-3.2-3-5.5"}]],link:[["path",{d:"M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"}],["path",{d:"M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"}]],orchestrator:[["rect",{x:"16",y:"16",width:"6",height:"6",rx:"1"}],["rect",{x:"2",y:"16",width:"6",height:"6",rx:"1"}],["rect",{x:"9",y:"2",width:"6",height:"6",rx:"1"}],["path",{d:"M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3"}],["path",{d:"M12 12V8"}]],search:[["path",{d:"m21 21-4.34-4.34"}],["circle",{cx:"11",cy:"11",r:"8"}]],external:[["path",{d:"M15 3h6v6"}],["path",{d:"M10 14 21 3"}],["path",{d:"M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"}]],clock:[["circle",{cx:"12",cy:"12",r:"10"}],["path",{d:"M12 6v6l4 2"}]],zap:[["path",{d:"M15.914 4a1.5 1.5 0 00-2.474-1.561l-9 9A1.5 1.5 0 005.5 14h4.002a.5.5 0 01.471.666L8.086 20a1.5 1.5 0 002.475 1.56l9-9A1.5 1.5 0 0018.5 10h-3.997a.5.5 0 01-.472-.667z"}]],file:[["path",{d:"M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"}],["path",{d:"M14 2v5a1 1 0 0 0 1 1h5"}],["path",{d:"M10 9H8"}],["path",{d:"M16 13H8"}],["path",{d:"M16 17H8"}]],settings:[["path",{d:"M10 5H3"}],["path",{d:"M12 19H3"}],["path",{d:"M14 3v4"}],["path",{d:"M16 17v4"}],["path",{d:"M21 12h-9"}],["path",{d:"M21 19h-5"}],["path",{d:"M21 5h-7"}],["path",{d:"M8 10v4"}],["path",{d:"M8 12H3"}]],ban:[["circle",{cx:"12",cy:"12",r:"10"}],["path",{d:"M4.929 4.929 19.07 19.071"}]],copy:[["rect",{width:"14",height:"14",x:"8",y:"8",rx:"2",ry:"2"}],["path",{d:"M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"}]],sparkles:[["path",{d:"M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z"}],["path",{d:"M20 2v4"}],["path",{d:"M22 4h-4"}],["circle",{cx:"4",cy:"20",r:"2"}]]};function y(n,e,...t){let[i,...r]=n.split("."),s=document.createElement(i);if(r.length&&(s.className=r.join(" ")),e)for(let[o,a]of Object.entries(e))a==null||a===!1||(o.startsWith("on")&&typeof a=="function"?s.addEventListener(o.slice(2),a):o==="class"?s.className=[s.className,String(a)].filter(Boolean).join(" "):a===!0?s.setAttribute(o,""):s.setAttribute(o,String(a)));return Ku(s,t),s}function Ku(n,e){for(let t of e.flat())t==null||t===!1||n.appendChild(typeof t=="object"?t:document.createTextNode(String(t)))}function Be(n,...e){for(;n.firstChild;)n.removeChild(n.firstChild);Ku(n,e)}var ko="http://www.w3.org/2000/svg";function pi(n,e=""){let t=document.createElementNS(ko,"svg");t.setAttribute("viewBox","0 0 24 24"),t.setAttribute("class",`icon ${e}`.trim()),t.setAttribute("aria-hidden","true");for(let[i,r]of Zu[n]){let s=document.createElementNS(ko,i);for(let[o,a]of Object.entries(r))s.setAttribute(o,a);t.appendChild(s)}return t}function Ju(n,e="0 0 24 24"){let t=document.createElementNS(ko,"svg");t.setAttribute("viewBox",e),t.setAttribute("aria-hidden","true");let i=document.createElementNS(ko,"path");return i.setAttribute("d",n),t.appendChild(i),t}function at(n,e="muted",t=!1){return y("span.badge",{class:e==="muted"?"":e},t?y("span.dot",{class:e}):null,n)}function mi(n){return y("span.dot",{class:n})}function Ze(n,e,t={}){let i=[t.kind,t.size,n?"":"icon-only"].filter(Boolean).join(" "),r=y("button.btn",{class:i,type:"button",title:t.title??null,disabled:!!t.disabled},t.icon?pi(t.icon):null,n||null);return r.addEventListener("click",e),r}function Xt(n,e,t={}){return y("section.card",null,n!=null?y("header.card-head",null,typeof n=="string"?y("h2",null,n):n,...t.actions??[]):null,y("div.card-body",{class:t.flush?"flush":""},e))}function ni(n,e,t){return y("div.card.stat",null,y("span.label",null,n),y("span.value",null,String(e)),t?y("span.sub",null,t):null)}function ft(n,e,t,i){return y("div.empty",null,pi(n),y("h3",null,e),t?y("p",null,t):null,i??null)}function ur(n,e,t){return y("table.table",null,y("thead",null,y("tr",null,n.map(i=>y("th",{class:i.num?"num":"",style:i.width?`width:${i.width}`:null},i.label)))),y("tbody",null,e.map(i=>{let r=y("tr",{class:t?"clickable":""},n.map(s=>y("td",{class:s.num?"num":""},s.cell(i))));return t&&r.addEventListener("click",()=>t(i)),r})))}function yt(n,e,t){return y("label.field",null,y("span",null,n),e,t?y("span.hint",null,t):null)}function jt(n={}){return y("input.input",n)}function ii(n={}){return y("textarea.textarea",n)}function gn(n,e){let t=y("select.select",null,n.map(i=>y("option",{value:i.value},i.label)));return e!==void 0&&(t.value=e),t}function ju(n,e,t){return y("div.tabs",{role:"tablist"},n.map(i=>{let r=y("button",{role:"tab","aria-selected":i.key===e?"true":"false"},i.label);return r.addEventListener("click",()=>t(i.key)),r}))}function Ln(n,e,t){let i=()=>{s.remove(),document.removeEventListener("keydown",r),t?.()},r=o=>{o.key==="Escape"&&i()},s=y("div.modal-backdrop",null,y("div.modal",{role:"dialog","aria-modal":"true","aria-label":n},y("header.card-head",null,y("h2",null,n),Ze("",i,{kind:"ghost",icon:"x",title:"Close"})),y("div.card-body",null,e)));return s.addEventListener("mousedown",o=>{o.target===s&&i()}),document.addEventListener("keydown",r),document.body.appendChild(s),s.querySelector("input, textarea, select, button.primary")?.focus(),i}function sn(n,e="info"){let t=document.querySelector(".toast-host");t||(t=y("div.toast-host",{"aria-live":"polite"}),document.body.appendChild(t));let i=y("div.toast",{class:e==="error"?"error":""},n);t.appendChild(i),setTimeout(()=>i.remove(),e==="error"?6e3:3e3)}async function pt(n,e){try{let t=await n();return e&&sn(e),t}catch(t){sn(t instanceof Error?t.message:String(t),"error");return}}var xp={zai:"Z",groq:"Gq",together:"Tg",fireworks:"Fw",cerebras:"Cb",llamacpp:"ll",mock:"M"};function Jl(n,e=!1){let t=y("span.logo",{class:e?"lg":"",title:n}),i=Yu[n];if(i){t.appendChild(Ju(i.path));let r=i.hex.toLowerCase(),s=parseInt(r.slice(0,2),16)+parseInt(r.slice(2,4),16)+parseInt(r.slice(4,6),16)<150;t.style.color=s?"var(--text)":`#${r}`}else Be(t,y("span.mono-mark",null,xp[n]??n.slice(0,2).toUpperCase()));return t}function qt(n,e=Date.now()){if(!n)return"\u2014";let t=Math.max(0,Math.round((e-n)/1e3));return t<5?"just now":t<60?`${t}s ago`:t<3600?`${Math.floor(t/60)}m ago`:t<86400?`${Math.floor(t/3600)}h ago`:`${Math.floor(t/86400)}d ago`}function Ur(n){return n<1e3?String(n):n<1e6?`${(n/1e3).toFixed(n<1e4?1:0)}k`:`${(n/1e6).toFixed(1)}M`}function Qt(n,e){return n.length>e?`${n.slice(0,e-1)}\u2026`:n}var vp={orchestrator:"Orchestrator",architect:"Architect",auditor:"Auditor",breeder:"Breeder",builder:"Builder","edge-operator":"Edge operator",finance:"Finance",incident:"Incident","legal-agents-counsel":"Legal \xB7 agents' counsel","legal-agents-defender":"Legal \xB7 agents' defender","legal-agents-ombudsman":"Legal \xB7 agents' ombudsman","legal-gov-judge":"Legal \xB7 government judge","legal-gov-prosecutor":"Legal \xB7 government prosecutor",librarian:"Librarian",mayor:"Mayor","mayor-office":"Mayor \xB7 office",planner:"Planner",qa:"QA",release:"Release",researcher:"Researcher",reviewer:"Reviewer","security-reviewer":"Security reviewer"};function xn(n){return vp[n]??n}function ln(n){return n.name.trim()===""?"Unnamed":n.name}var yp=80;function Qu(n,e){let t=y("header.agent-head"),i=y("div.transcript"),r=ii({placeholder:"Message this agent\u2026",rows:3}),s=Ze("Send",()=>{I()},{kind:"primary",icon:"send",title:"Send (Ctrl+Enter)"}),o=y("select.select"),a=y("div.model-line"),l=y("div"),c=y("div.stack"),h=new Map,d=new Map,u=[],p,x=!0,v=!1,m=!1,f=0,b,P=U=>{let z=i.scrollHeight-i.scrollTop-i.clientHeight<yp;U(),z&&(i.scrollTop=i.scrollHeight)},w=U=>{let z=h.get(U);if(z)return z;let Y=y("header.work-head"),ee=y("section.work",null,Y),J={root:ee,head:Y};return h.set(U,J),i.append(ee),J},T=U=>{for(let[z,Y]of d)Y.work===U&&(Y.el.remove(),d.delete(z))},E=U=>{if(U.seq<=f||(f=U.seq,U.kind==="model_request"))return;let z=U.data;P(()=>{let Y=w(U.work);switch(U.kind){case"work_start":Be(Y.head,at(String(z.kind??"work"),"info"),y("p.work-prompt",null,String(z.prompt??"")));break;case"work_end":{let ee=String(z.status??""),J=/fail|error/.test(ee)?"danger":/cancel|stop/.test(ee)?"warn":"ok";Y.root.append(y("div.work-end",null,at(ee||"done",J)));break}case"model_response":T(U.work),Y.root.append(..._p(z));break;case"tool_decision":{let ee=bp(z);ee&&Y.root.append(y("div.decision",null,ee));break}case"tool_result":{let ee=z.is_error===!0,J=String(z.content??z.output??"");Y.root.append(y("details.result",null,y("summary",{class:ee?"danger":"ok"},ee?"Error":"Output"),y("pre.pre",null,J)));break}case"error":Y.root.append(y("p.danger",null,String(z.message??"Error")));break;default:break}})},D=U=>{let z=U;if(z.cid!==e)return;let Y=`${z.work}:${z.req}`;P(()=>{let ee=d.get(Y);if(!ee){let ie=new Text,ce=new Text,Se=y("div.live-turn",null,y("p.prose",null,ie));ee={work:z.work,el:Se,text:ie,thinking:ce},d.set(Y,ee),w(z.work).root.append(Se)}let J=z.delta;J.type==="tool_use"?ee.el.append(y("span.chip",null,`\u2192 ${J.name}`)):J.type==="text"&&ee.text.appendData(J.text)})},g=async()=>{let U=await pt(()=>ze.transcript(e));if(x){for(let z of(U??[]).slice().sort((Y,ee)=>Y.seq-ee.seq))E(z);v=!0;for(let z of u.splice(0))z();i.scrollTop=i.scrollHeight}},S=U=>{let z=b??"",Y=U.map(ee=>({value:`${ee.provider}/${ee.id}`,label:ee.name??ee.id}));z&&!Y.some(ee=>ee.value===z)&&Y.push({value:z,label:z}),o.replaceChildren(y("option",{value:""},"Use the default"),...Y.map(ee=>y("option",{value:ee.value},ee.label))),o.value=z},_=()=>{let U=p;if(!U)return;let z=U.status==="offline",Y=U.status==="working"?"Working":U.status==="idle"?"Idle":"Offline",ee=U.status==="working"?"info":U.status==="idle"?"ok":"muted";Be(t,y("a.back",{href:bn("agents")},"Agents"),y("div.agent-title",null,y("h1",null,ln(U)),at(Y,ee,!0),at(U.tier==="orchestrator"?"Orchestrator":"Worker",U.tier==="orchestrator"?"accent":"muted"),y("span.muted",null,xn(U.role))),y("div.row.agent-actions",null,U.status==="working"?Ze("Cancel work",()=>{pt(()=>ze.cancel(e),"Cancelled")},{icon:"ban"}):null,Ze("Stop",()=>{pt(()=>ze.stop(e),"Stopped")},{kind:"danger",icon:"stop"}))),r.disabled=z,s.disabled=z||m;let J=U.model_source==="agent"?"set for this agent":U.model_source==="role"?"from role":"default";Be(a,y("strong",null,U.model),y("span.faint",null,J));let ie=U.model_override??"";ie!==b&&(b=ie,o.value=ie),Be(l,y("dl.kv",null,y("dt",null,"CID"),y("dd.mono",null,String(U.cid)),y("dt",null,"Run"),y("dd.mono",null,U.run??"\u2014"),y("dt",null,"VM"),y("dd.mono",null,U.vm===null?"\u2014":String(U.vm)),y("dt",null,"Connected"),y("dd",null,qt(U.connected_at_ms)),y("dt",null,"Last seen"),y("dd",null,qt(U.last_seen_ms)),y("dt",null,"Tokens in"),y("dd.mono",null,Ur(U.usage.input_tokens)),y("dt",null,"Tokens out"),y("dd.mono",null,Ur(U.usage.output_tokens)),y("dt",null,"Credits"),y("dd.mono",null,String(U.credits)))),Be(c,U.work?y("div.row",null,at(U.work.kind,"info"),y("span.truncate",null,Qt(U.work.prompt,160))):y("span.faint",null,"Nothing running"),y("span.muted",null,`${U.queue} queued`))},A=()=>p?.status==="offline",I=async()=>{let U=r.value.trim();if(!U||m||A())return;m=!0,s.disabled=!0;let z=await pt(()=>ze.chat(e,U));m=!1,s.disabled=A(),z&&(r.value="")};r.addEventListener("keydown",U=>{U.key==="Enter"&&(U.metaKey||U.ctrlKey)&&(U.preventDefault(),I())}),o.addEventListener("change",()=>{let U=o.value;pt(()=>ze.setAgentModel(e,U||null),"Model updated").then(z=>{z?b=U:o.value=b??""})});let k=[dt.watch(U=>{let z=U.find(Y=>Y.cid===e);z&&(p=z,_())}),Mt("transcript",U=>{let z=U;z.cid===e&&(v?E(z):u.push(()=>E(z)))}),Mt("delta",U=>{U.cid===e&&(v?D(U):u.push(()=>D(U)))})];return n.append(y("div.agent-view",null,t,y("div.agent-grid",null,y("div.agent-main",null,Xt(null,i,{flush:!0}),y("div.composer",null,r,y("div.row",null,y("span",{style:"flex:1"}),s))),y("aside.agent-side",null,Xt("Model",y("div.stack",null,a,yt("Change model",o))),Xt("Details",l),Xt("Queue",c))))),(async()=>{try{p=await ze.agent(e)}catch(U){if(!x)return;Be(n,U instanceof Ts&&U.status===404?ft("agents","No such agent",void 0,y("a.btn",{href:bn("agents")},"All agents")):ft("alert","Could not load this agent",U instanceof Error?U.message:void 0));return}x&&(_(),ze.models().then(U=>x&&S(U.models)).catch(()=>{}),await g())})(),()=>{x=!1,k.forEach(U=>U()),d.clear()}}function _p(n){let e=Array.isArray(n.content)?n.content:[],t=[];for(let i of e)i.type==="text"?t.push(y("p.prose",null,i.text)):i.type==="tool_use"&&t.push(y("details.tool",null,y("summary.chip",null,`\u2192 ${i.name}`),y("pre.pre",null,JSON.stringify(i.input,null,2))));return t}function bp(n){switch(n.decision){case"allow":return at("ran in VM","info");case"done":return at("ran on host","muted");case"deny":return at(`denied: ${String(n.reason??"")}`,"danger");default:return null}}var Qi={LEFT:0,MIDDLE:1,RIGHT:2,ROTATE:0,DOLLY:1,PAN:2},er={ROTATE:0,PAN:1,DOLLY_PAN:2,DOLLY_ROTATE:3},Rh=0,Cc=1,Ch=2;var Mr=1,Ph=2,hs=3,tr=0,mn=1,Fn=2,_i=0,ds=1,Li=2,Pc=3,Ic=4,Ih=5;var Sr=100,kh=101,Lh=102,Dh=103,Nh=104,Uh=200,Fh=201,Oh=202,Bh=203,kc=204,Lc=205,Hh=206,zh=207,Gh=208,Vh=209,Wh=210,Xh=211,qh=212,$h=213,Yh=214,ia=0,ra=1,sa=2,Qr=3,oa=4,aa=5,la=6,ca=7,Dc=0,Zh=1,Kh=2,ci=0,Nc=1,Uc=2,Fc=3,Oc=4,Bc=5,Hc=6,zc=7;var Gc=300,nr=301,wr=302,ka=303,La=304,so=306,gr=1e3,xi=1001,ua=1002,en=1003,Jh=1004;var oo=1005;var vn=1006,Da=1007;var ir=1008;var On=1009,Vc=1010,Wc=1011,fs=1012,Na=1013,ui=1014,Jn=1015,hi=1016,Ua=1017,Fa=1018,ps=1020,Xc=35902,qc=35899,$c=1021,Yc=1022,Bn=1023,vi=1026,rr=1027,Oa=1028,Ba=1029,sr=1030,Ha=1031;var za=1033,ao=33776,lo=33777,co=33778,uo=33779,Ga=35840,Va=35841,Wa=35842,Xa=35843,qa=36196,$a=37492,Ya=37496,Za=37488,Ka=37489,ho=37490,Ja=37491,ja=37808,Qa=37809,el=37810,tl=37811,nl=37812,il=37813,rl=37814,sl=37815,ol=37816,al=37817,ll=37818,cl=37819,ul=37820,hl=37821,dl=36492,fl=36494,pl=36495,ml=36283,gl=36284,fo=36285,xl=36286;var Fs=2300,ha=2301,ta=2302,yc=2303,_c=2400,bc=2401,Mc=2402;var jh=3200;var vl=0,Qh=1,Di="",Pt="srgb",Os="srgb-linear",Bs="linear",St="srgb";var na=7680;var ed=519,td=512,nd=513,id=514,yl=515,rd=516,sd=517,_l=518,od=519,ad=35044;var Zc="300 es",ai=2e3,es=2001;function Mp(n){for(let e=n.length-1;e>=0;--e)if(n[e]>=65535)return!0;return!1}function Sp(n){return ArrayBuffer.isView(n)&&!(n instanceof DataView)}function Hs(n){return document.createElementNS("http://www.w3.org/1999/xhtml",n)}function ld(){let n=Hs("canvas");return n.style.display="block",n}var eh={},ts=null;function Kc(...n){let e="THREE."+n.shift();ts?ts("log",e,...n):console.log(e,...n)}function cd(n){let e=n[0];if(typeof e=="string"&&e.startsWith("TSL:")){let t=n[1];t&&t.isStackTrace?n[0]+=" "+t.getLocation():n[1]='Stack trace not available. Enable "THREE.Node.captureStackTrace" to capture stack traces.'}return n}function Ye(...n){n=cd(n);let e="THREE."+n.shift();if(ts)ts("warn",e,...n);else{let t=n[0];t&&t.isStackTrace?console.warn(t.getError(e)):console.warn(e,...n)}}function Ke(...n){n=cd(n);let e="THREE."+n.shift();if(ts)ts("error",e,...n);else{let t=n[0];t&&t.isStackTrace?console.error(t.getError(e)):console.error(e,...n)}}function mr(...n){let e=n.join(" ");e in eh||(eh[e]=!0,Ye(...n))}function ud(n,e,t){return new Promise(function(i,r){function s(){switch(n.clientWaitSync(e,n.SYNC_FLUSH_COMMANDS_BIT,0)){case n.WAIT_FAILED:r();break;case n.TIMEOUT_EXPIRED:setTimeout(s,t);break;default:i()}}setTimeout(s,t)})}var hd={[ia]:ra,[sa]:la,[oa]:ca,[Qr]:aa,[ra]:ia,[la]:sa,[ca]:oa,[aa]:Qr},li=class{addEventListener(e,t){this._listeners===void 0&&(this._listeners={});let i=this._listeners;i[e]===void 0&&(i[e]=[]),i[e].indexOf(t)===-1&&i[e].push(t)}hasEventListener(e,t){let i=this._listeners;return i===void 0?!1:i[e]!==void 0&&i[e].indexOf(t)!==-1}removeEventListener(e,t){let i=this._listeners;if(i===void 0)return;let r=i[e];if(r!==void 0){let s=r.indexOf(t);s!==-1&&r.splice(s,1)}}dispatchEvent(e){let t=this._listeners;if(t===void 0)return;let i=t[e.type];if(i!==void 0){e.target=this;let r=i.slice(0);for(let s=0,o=r.length;s<o;s++)r[s].call(this,e);e.target=null}}},Sn=["00","01","02","03","04","05","06","07","08","09","0a","0b","0c","0d","0e","0f","10","11","12","13","14","15","16","17","18","19","1a","1b","1c","1d","1e","1f","20","21","22","23","24","25","26","27","28","29","2a","2b","2c","2d","2e","2f","30","31","32","33","34","35","36","37","38","39","3a","3b","3c","3d","3e","3f","40","41","42","43","44","45","46","47","48","49","4a","4b","4c","4d","4e","4f","50","51","52","53","54","55","56","57","58","59","5a","5b","5c","5d","5e","5f","60","61","62","63","64","65","66","67","68","69","6a","6b","6c","6d","6e","6f","70","71","72","73","74","75","76","77","78","79","7a","7b","7c","7d","7e","7f","80","81","82","83","84","85","86","87","88","89","8a","8b","8c","8d","8e","8f","90","91","92","93","94","95","96","97","98","99","9a","9b","9c","9d","9e","9f","a0","a1","a2","a3","a4","a5","a6","a7","a8","a9","aa","ab","ac","ad","ae","af","b0","b1","b2","b3","b4","b5","b6","b7","b8","b9","ba","bb","bc","bd","be","bf","c0","c1","c2","c3","c4","c5","c6","c7","c8","c9","ca","cb","cc","cd","ce","cf","d0","d1","d2","d3","d4","d5","d6","d7","d8","d9","da","db","dc","dd","de","df","e0","e1","e2","e3","e4","e5","e6","e7","e8","e9","ea","eb","ec","ed","ee","ef","f0","f1","f2","f3","f4","f5","f6","f7","f8","f9","fa","fb","fc","fd","fe","ff"],th=1234567,Ns=Math.PI/180,ns=180/Math.PI;function ms(){let n=Math.random()*4294967295|0,e=Math.random()*4294967295|0,t=Math.random()*4294967295|0,i=Math.random()*4294967295|0;return(Sn[n&255]+Sn[n>>8&255]+Sn[n>>16&255]+Sn[n>>24&255]+"-"+Sn[e&255]+Sn[e>>8&255]+"-"+Sn[e>>16&15|64]+Sn[e>>24&255]+"-"+Sn[t&63|128]+Sn[t>>8&255]+"-"+Sn[t>>16&255]+Sn[t>>24&255]+Sn[i&255]+Sn[i>>8&255]+Sn[i>>16&255]+Sn[i>>24&255]).toLowerCase()}function lt(n,e,t){return Math.max(e,Math.min(t,n))}function Jc(n,e){return(n%e+e)%e}function wp(n,e,t,i,r){return i+(n-e)*(r-i)/(t-e)}function Ep(n,e,t){return n!==e?(t-n)/(e-n):0}function Us(n,e,t){return(1-t)*n+t*e}function Tp(n,e,t,i){return Us(n,e,1-Math.exp(-t*i))}function Ap(n,e=1){return e-Math.abs(Jc(n,e*2)-e)}function Rp(n,e,t){return n<=e?0:n>=t?1:(n=(n-e)/(t-e),n*n*(3-2*n))}function Cp(n,e,t){return n<=e?0:n>=t?1:(n=(n-e)/(t-e),n*n*n*(n*(n*6-15)+10))}function Pp(n,e){return n+Math.floor(Math.random()*(e-n+1))}function Ip(n,e){return n+Math.random()*(e-n)}function kp(n){return n*(.5-Math.random())}function Lp(n){n!==void 0&&(th=n);let e=th+=1831565813;return e=Math.imul(e^e>>>15,e|1),e^=e+Math.imul(e^e>>>7,e|61),((e^e>>>14)>>>0)/4294967296}function Dp(n){return n*Ns}function Np(n){return n*ns}function Up(n){return n>0&&Number.isInteger(n)&&2**Math.round(Math.log2(n))===n}function Fp(n){return Math.pow(2,Math.ceil(Math.log(n)/Math.LN2))}function Op(n){return Math.pow(2,Math.floor(Math.log(n)/Math.LN2))}function Bp(n,e,t,i,r){let s=Math.cos,o=Math.sin,a=s(t/2),l=o(t/2),c=s((e+i)/2),h=o((e+i)/2),d=s((e-i)/2),u=o((e-i)/2),p=s((i-e)/2),x=o((i-e)/2);switch(r){case"XYX":n.set(a*h,l*d,l*u,a*c);break;case"YZY":n.set(l*u,a*h,l*d,a*c);break;case"ZXZ":n.set(l*d,l*u,a*h,a*c);break;case"XZX":n.set(a*h,l*x,l*p,a*c);break;case"YXY":n.set(l*p,a*h,l*x,a*c);break;case"ZYZ":n.set(l*x,l*p,a*h,a*c);break;default:Ye("MathUtils: .setQuaternionFromProperEuler() encountered an unknown order: "+r)}}function Jr(n,e){switch(e.constructor){case Float32Array:return n;case Uint32Array:return n/4294967295;case Uint16Array:return n/65535;case Uint8Array:case Uint8ClampedArray:return n/255;case Int32Array:return Math.max(n/2147483647,-1);case Int16Array:return Math.max(n/32767,-1);case Int8Array:return Math.max(n/127,-1);default:throw new Error("THREE.MathUtils: Invalid component type.")}}function Rn(n,e){switch(e.constructor){case Float32Array:return n;case Uint32Array:return Math.round(n*4294967295);case Uint16Array:return Math.round(n*65535);case Uint8Array:case Uint8ClampedArray:return Math.round(n*255);case Int32Array:return Math.round(n*2147483647);case Int16Array:return Math.round(n*32767);case Int8Array:return Math.round(n*127);default:throw new Error("THREE.MathUtils: Invalid component type.")}}var jc={DEG2RAD:Ns,RAD2DEG:ns,generateUUID:ms,clamp:lt,euclideanModulo:Jc,mapLinear:wp,inverseLerp:Ep,lerp:Us,damp:Tp,pingpong:Ap,smoothstep:Rp,smootherstep:Cp,randInt:Pp,randFloat:Ip,randFloatSpread:kp,seededRandom:Lp,degToRad:Dp,radToDeg:Np,isPowerOfTwo:Up,ceilPowerOfTwo:Fp,floorPowerOfTwo:Op,setQuaternionFromProperEuler:Bp,normalize:Rn,denormalize:Jr},Xe=class n{static{n.prototype.isVector2=!0}constructor(e=0,t=0){this.x=e,this.y=t}get width(){return this.x}set width(e){this.x=e}get height(){return this.y}set height(e){this.y=e}set(e,t){return this.x=e,this.y=t,this}setScalar(e){return this.x=e,this.y=e,this}setX(e){return this.x=e,this}setY(e){return this.y=e,this}setComponent(e,t){switch(e){case 0:this.x=t;break;case 1:this.y=t;break;default:throw new Error("THREE.Vector2: index is out of range: "+e)}return this}getComponent(e){switch(e){case 0:return this.x;case 1:return this.y;default:throw new Error("THREE.Vector2: index is out of range: "+e)}}clone(){return new this.constructor(this.x,this.y)}copy(e){return this.x=e.x,this.y=e.y,this}add(e){return this.x+=e.x,this.y+=e.y,this}addScalar(e){return this.x+=e,this.y+=e,this}addVectors(e,t){return this.x=e.x+t.x,this.y=e.y+t.y,this}addScaledVector(e,t){return this.x+=e.x*t,this.y+=e.y*t,this}sub(e){return this.x-=e.x,this.y-=e.y,this}subScalar(e){return this.x-=e,this.y-=e,this}subVectors(e,t){return this.x=e.x-t.x,this.y=e.y-t.y,this}multiply(e){return this.x*=e.x,this.y*=e.y,this}multiplyScalar(e){return this.x*=e,this.y*=e,this}divide(e){return this.x/=e.x,this.y/=e.y,this}divideScalar(e){return this.multiplyScalar(1/e)}applyMatrix3(e){let t=this.x,i=this.y,r=e.elements;return this.x=r[0]*t+r[3]*i+r[6],this.y=r[1]*t+r[4]*i+r[7],this}min(e){return this.x=Math.min(this.x,e.x),this.y=Math.min(this.y,e.y),this}max(e){return this.x=Math.max(this.x,e.x),this.y=Math.max(this.y,e.y),this}clamp(e,t){return this.x=lt(this.x,e.x,t.x),this.y=lt(this.y,e.y,t.y),this}clampScalar(e,t){return this.x=lt(this.x,e,t),this.y=lt(this.y,e,t),this}clampLength(e,t){let i=this.length();return this.divideScalar(i||1).multiplyScalar(lt(i,e,t))}floor(){return this.x=Math.floor(this.x),this.y=Math.floor(this.y),this}ceil(){return this.x=Math.ceil(this.x),this.y=Math.ceil(this.y),this}round(){return this.x=Math.round(this.x),this.y=Math.round(this.y),this}roundToZero(){return this.x=Math.trunc(this.x),this.y=Math.trunc(this.y),this}negate(){return this.x=-this.x,this.y=-this.y,this}dot(e){return this.x*e.x+this.y*e.y}cross(e){return this.x*e.y-this.y*e.x}lengthSq(){return this.x*this.x+this.y*this.y}length(){return Math.sqrt(this.x*this.x+this.y*this.y)}manhattanLength(){return Math.abs(this.x)+Math.abs(this.y)}normalize(){return this.divideScalar(this.length()||1)}angle(){return Math.atan2(-this.y,-this.x)+Math.PI}angleTo(e){let t=Math.sqrt(this.lengthSq()*e.lengthSq());if(t===0)return Math.PI/2;let i=this.dot(e)/t;return Math.acos(lt(i,-1,1))}distanceTo(e){return Math.sqrt(this.distanceToSquared(e))}distanceToSquared(e){let t=this.x-e.x,i=this.y-e.y;return t*t+i*i}manhattanDistanceTo(e){return Math.abs(this.x-e.x)+Math.abs(this.y-e.y)}setLength(e){return this.normalize().multiplyScalar(e)}lerp(e,t){return this.x+=(e.x-this.x)*t,this.y+=(e.y-this.y)*t,this}lerpVectors(e,t,i){return this.x=e.x+(t.x-e.x)*i,this.y=e.y+(t.y-e.y)*i,this}equals(e){return e.x===this.x&&e.y===this.y}fromArray(e,t=0){return this.x=e[t],this.y=e[t+1],this}toArray(e=[],t=0){return e[t]=this.x,e[t+1]=this.y,e}fromBufferAttribute(e,t){return this.x=e.getX(t),this.y=e.getY(t),this}rotateAround(e,t){let i=Math.cos(t),r=Math.sin(t),s=this.x-e.x,o=this.y-e.y;return this.x=s*i-o*r+e.x,this.y=s*r+o*i+e.y,this}random(){return this.x=Math.random(),this.y=Math.random(),this}*[Symbol.iterator](){yield this.x,yield this.y}},un=class{constructor(e=0,t=0,i=0,r=1){this.isQuaternion=!0,this._x=e,this._y=t,this._z=i,this._w=r}static slerpFlat(e,t,i,r,s,o,a){let l=i[r+0],c=i[r+1],h=i[r+2],d=i[r+3],u=s[o+0],p=s[o+1],x=s[o+2],v=s[o+3];if(d!==v||l!==u||c!==p||h!==x){let m=l*u+c*p+h*x+d*v;m<0&&(u=-u,p=-p,x=-x,v=-v,m=-m);let f=1-a;if(m<.9995){let b=Math.acos(m),P=Math.sin(b);f=Math.sin(f*b)/P,a=Math.sin(a*b)/P,l=l*f+u*a,c=c*f+p*a,h=h*f+x*a,d=d*f+v*a}else{l=l*f+u*a,c=c*f+p*a,h=h*f+x*a,d=d*f+v*a;let b=1/Math.sqrt(l*l+c*c+h*h+d*d);l*=b,c*=b,h*=b,d*=b}}e[t]=l,e[t+1]=c,e[t+2]=h,e[t+3]=d}static multiplyQuaternionsFlat(e,t,i,r,s,o){let a=i[r],l=i[r+1],c=i[r+2],h=i[r+3],d=s[o],u=s[o+1],p=s[o+2],x=s[o+3];return e[t]=a*x+h*d+l*p-c*u,e[t+1]=l*x+h*u+c*d-a*p,e[t+2]=c*x+h*p+a*u-l*d,e[t+3]=h*x-a*d-l*u-c*p,e}get x(){return this._x}set x(e){this._x=e,this._onChangeCallback()}get y(){return this._y}set y(e){this._y=e,this._onChangeCallback()}get z(){return this._z}set z(e){this._z=e,this._onChangeCallback()}get w(){return this._w}set w(e){this._w=e,this._onChangeCallback()}set(e,t,i,r){return this._x=e,this._y=t,this._z=i,this._w=r,this._onChangeCallback(),this}clone(){return new this.constructor(this._x,this._y,this._z,this._w)}copy(e){return this._x=e.x,this._y=e.y,this._z=e.z,this._w=e.w,this._onChangeCallback(),this}setFromEuler(e,t=!0){let i=e._x,r=e._y,s=e._z,o=e._order,a=Math.cos,l=Math.sin,c=a(i/2),h=a(r/2),d=a(s/2),u=l(i/2),p=l(r/2),x=l(s/2);switch(o){case"XYZ":this._x=u*h*d+c*p*x,this._y=c*p*d-u*h*x,this._z=c*h*x+u*p*d,this._w=c*h*d-u*p*x;break;case"YXZ":this._x=u*h*d+c*p*x,this._y=c*p*d-u*h*x,this._z=c*h*x-u*p*d,this._w=c*h*d+u*p*x;break;case"ZXY":this._x=u*h*d-c*p*x,this._y=c*p*d+u*h*x,this._z=c*h*x+u*p*d,this._w=c*h*d-u*p*x;break;case"ZYX":this._x=u*h*d-c*p*x,this._y=c*p*d+u*h*x,this._z=c*h*x-u*p*d,this._w=c*h*d+u*p*x;break;case"YZX":this._x=u*h*d+c*p*x,this._y=c*p*d+u*h*x,this._z=c*h*x-u*p*d,this._w=c*h*d-u*p*x;break;case"XZY":this._x=u*h*d-c*p*x,this._y=c*p*d-u*h*x,this._z=c*h*x+u*p*d,this._w=c*h*d+u*p*x;break;default:Ye("Quaternion: .setFromEuler() encountered an unknown order: "+o)}return t===!0&&this._onChangeCallback(),this}setFromAxisAngle(e,t){let i=t/2,r=Math.sin(i);return this._x=e.x*r,this._y=e.y*r,this._z=e.z*r,this._w=Math.cos(i),this._onChangeCallback(),this}setFromRotationMatrix(e){let t=e.elements,i=t[0],r=t[4],s=t[8],o=t[1],a=t[5],l=t[9],c=t[2],h=t[6],d=t[10],u=i+a+d;if(u>0){let p=.5/Math.sqrt(u+1);this._w=.25/p,this._x=(h-l)*p,this._y=(s-c)*p,this._z=(o-r)*p}else if(i>a&&i>d){let p=2*Math.sqrt(1+i-a-d);this._w=(h-l)/p,this._x=.25*p,this._y=(r+o)/p,this._z=(s+c)/p}else if(a>d){let p=2*Math.sqrt(1+a-i-d);this._w=(s-c)/p,this._x=(r+o)/p,this._y=.25*p,this._z=(l+h)/p}else{let p=2*Math.sqrt(1+d-i-a);this._w=(o-r)/p,this._x=(s+c)/p,this._y=(l+h)/p,this._z=.25*p}return this._onChangeCallback(),this}setFromUnitVectors(e,t){let i=e.dot(t)+1;return i<1e-8?(i=0,Math.abs(e.x)>Math.abs(e.z)?(this._x=-e.y,this._y=e.x,this._z=0,this._w=i):(this._x=0,this._y=-e.z,this._z=e.y,this._w=i)):(this._x=e.y*t.z-e.z*t.y,this._y=e.z*t.x-e.x*t.z,this._z=e.x*t.y-e.y*t.x,this._w=i),this.normalize()}angleTo(e){return 2*Math.acos(Math.abs(lt(this.dot(e),-1,1)))}rotateTowards(e,t){let i=this.angleTo(e);if(i===0)return this;let r=Math.min(1,t/i);return this.slerp(e,r),this}identity(){return this.set(0,0,0,1)}invert(){return this.conjugate()}conjugate(){return this._x*=-1,this._y*=-1,this._z*=-1,this._onChangeCallback(),this}dot(e){return this._x*e._x+this._y*e._y+this._z*e._z+this._w*e._w}lengthSq(){return this._x*this._x+this._y*this._y+this._z*this._z+this._w*this._w}length(){return Math.sqrt(this._x*this._x+this._y*this._y+this._z*this._z+this._w*this._w)}normalize(){let e=this.length();return e===0?(this._x=0,this._y=0,this._z=0,this._w=1):(e=1/e,this._x=this._x*e,this._y=this._y*e,this._z=this._z*e,this._w=this._w*e),this._onChangeCallback(),this}multiply(e){return this.multiplyQuaternions(this,e)}premultiply(e){return this.multiplyQuaternions(e,this)}multiplyQuaternions(e,t){let i=e._x,r=e._y,s=e._z,o=e._w,a=t._x,l=t._y,c=t._z,h=t._w;return this._x=i*h+o*a+r*c-s*l,this._y=r*h+o*l+s*a-i*c,this._z=s*h+o*c+i*l-r*a,this._w=o*h-i*a-r*l-s*c,this._onChangeCallback(),this}slerp(e,t){let i=e._x,r=e._y,s=e._z,o=e._w,a=this.dot(e);a<0&&(i=-i,r=-r,s=-s,o=-o,a=-a);let l=1-t;if(a<.9995){let c=Math.acos(a),h=Math.sin(c);l=Math.sin(l*c)/h,t=Math.sin(t*c)/h,this._x=this._x*l+i*t,this._y=this._y*l+r*t,this._z=this._z*l+s*t,this._w=this._w*l+o*t,this._onChangeCallback()}else this._x=this._x*l+i*t,this._y=this._y*l+r*t,this._z=this._z*l+s*t,this._w=this._w*l+o*t,this.normalize();return this}slerpQuaternions(e,t,i){return this.copy(e).slerp(t,i)}random(){let e=2*Math.PI*Math.random(),t=2*Math.PI*Math.random(),i=Math.random(),r=Math.sqrt(1-i),s=Math.sqrt(i);return this.set(r*Math.sin(e),r*Math.cos(e),s*Math.sin(t),s*Math.cos(t))}equals(e){return e._x===this._x&&e._y===this._y&&e._z===this._z&&e._w===this._w}fromArray(e,t=0){return this._x=e[t],this._y=e[t+1],this._z=e[t+2],this._w=e[t+3],this._onChangeCallback(),this}toArray(e=[],t=0){return e[t]=this._x,e[t+1]=this._y,e[t+2]=this._z,e[t+3]=this._w,e}fromBufferAttribute(e,t){return this._x=e.getX(t),this._y=e.getY(t),this._z=e.getZ(t),this._w=e.getW(t),this._onChangeCallback(),this}toJSON(){return this.toArray()}_onChange(e){return this._onChangeCallback=e,this}_onChangeCallback(){}*[Symbol.iterator](){yield this._x,yield this._y,yield this._z,yield this._w}},G=class n{static{n.prototype.isVector3=!0}constructor(e=0,t=0,i=0){this.x=e,this.y=t,this.z=i}set(e,t,i){return i===void 0&&(i=this.z),this.x=e,this.y=t,this.z=i,this}setScalar(e){return this.x=e,this.y=e,this.z=e,this}setX(e){return this.x=e,this}setY(e){return this.y=e,this}setZ(e){return this.z=e,this}setComponent(e,t){switch(e){case 0:this.x=t;break;case 1:this.y=t;break;case 2:this.z=t;break;default:throw new Error("THREE.Vector3: index is out of range: "+e)}return this}getComponent(e){switch(e){case 0:return this.x;case 1:return this.y;case 2:return this.z;default:throw new Error("THREE.Vector3: index is out of range: "+e)}}clone(){return new this.constructor(this.x,this.y,this.z)}copy(e){return this.x=e.x,this.y=e.y,this.z=e.z,this}add(e){return this.x+=e.x,this.y+=e.y,this.z+=e.z,this}addScalar(e){return this.x+=e,this.y+=e,this.z+=e,this}addVectors(e,t){return this.x=e.x+t.x,this.y=e.y+t.y,this.z=e.z+t.z,this}addScaledVector(e,t){return this.x+=e.x*t,this.y+=e.y*t,this.z+=e.z*t,this}sub(e){return this.x-=e.x,this.y-=e.y,this.z-=e.z,this}subScalar(e){return this.x-=e,this.y-=e,this.z-=e,this}subVectors(e,t){return this.x=e.x-t.x,this.y=e.y-t.y,this.z=e.z-t.z,this}multiply(e){return this.x*=e.x,this.y*=e.y,this.z*=e.z,this}multiplyScalar(e){return this.x*=e,this.y*=e,this.z*=e,this}multiplyVectors(e,t){return this.x=e.x*t.x,this.y=e.y*t.y,this.z=e.z*t.z,this}applyEuler(e){return this.applyQuaternion(nh.setFromEuler(e))}applyAxisAngle(e,t){return this.applyQuaternion(nh.setFromAxisAngle(e,t))}applyMatrix3(e){let t=this.x,i=this.y,r=this.z,s=e.elements;return this.x=s[0]*t+s[3]*i+s[6]*r,this.y=s[1]*t+s[4]*i+s[7]*r,this.z=s[2]*t+s[5]*i+s[8]*r,this}applyNormalMatrix(e){return this.applyMatrix3(e).normalize()}applyMatrix4(e){let t=this.x,i=this.y,r=this.z,s=e.elements,o=1/(s[3]*t+s[7]*i+s[11]*r+s[15]);return this.x=(s[0]*t+s[4]*i+s[8]*r+s[12])*o,this.y=(s[1]*t+s[5]*i+s[9]*r+s[13])*o,this.z=(s[2]*t+s[6]*i+s[10]*r+s[14])*o,this}applyQuaternion(e){let t=this.x,i=this.y,r=this.z,s=e.x,o=e.y,a=e.z,l=e.w,c=2*(o*r-a*i),h=2*(a*t-s*r),d=2*(s*i-o*t);return this.x=t+l*c+o*d-a*h,this.y=i+l*h+a*c-s*d,this.z=r+l*d+s*h-o*c,this}project(e){return this.applyMatrix4(e.matrixWorldInverse).applyMatrix4(e.projectionMatrix)}unproject(e){return this.applyMatrix4(e.projectionMatrixInverse).applyMatrix4(e.matrixWorld)}transformDirection(e){let t=this.x,i=this.y,r=this.z,s=e.elements;return this.x=s[0]*t+s[4]*i+s[8]*r,this.y=s[1]*t+s[5]*i+s[9]*r,this.z=s[2]*t+s[6]*i+s[10]*r,this.normalize()}divide(e){return this.x/=e.x,this.y/=e.y,this.z/=e.z,this}divideScalar(e){return this.multiplyScalar(1/e)}min(e){return this.x=Math.min(this.x,e.x),this.y=Math.min(this.y,e.y),this.z=Math.min(this.z,e.z),this}max(e){return this.x=Math.max(this.x,e.x),this.y=Math.max(this.y,e.y),this.z=Math.max(this.z,e.z),this}clamp(e,t){return this.x=lt(this.x,e.x,t.x),this.y=lt(this.y,e.y,t.y),this.z=lt(this.z,e.z,t.z),this}clampScalar(e,t){return this.x=lt(this.x,e,t),this.y=lt(this.y,e,t),this.z=lt(this.z,e,t),this}clampLength(e,t){let i=this.length();return this.divideScalar(i||1).multiplyScalar(lt(i,e,t))}floor(){return this.x=Math.floor(this.x),this.y=Math.floor(this.y),this.z=Math.floor(this.z),this}ceil(){return this.x=Math.ceil(this.x),this.y=Math.ceil(this.y),this.z=Math.ceil(this.z),this}round(){return this.x=Math.round(this.x),this.y=Math.round(this.y),this.z=Math.round(this.z),this}roundToZero(){return this.x=Math.trunc(this.x),this.y=Math.trunc(this.y),this.z=Math.trunc(this.z),this}negate(){return this.x=-this.x,this.y=-this.y,this.z=-this.z,this}dot(e){return this.x*e.x+this.y*e.y+this.z*e.z}lengthSq(){return this.x*this.x+this.y*this.y+this.z*this.z}length(){return Math.sqrt(this.x*this.x+this.y*this.y+this.z*this.z)}manhattanLength(){return Math.abs(this.x)+Math.abs(this.y)+Math.abs(this.z)}normalize(){return this.divideScalar(this.length()||1)}setLength(e){return this.normalize().multiplyScalar(e)}lerp(e,t){return this.x+=(e.x-this.x)*t,this.y+=(e.y-this.y)*t,this.z+=(e.z-this.z)*t,this}lerpVectors(e,t,i){return this.x=e.x+(t.x-e.x)*i,this.y=e.y+(t.y-e.y)*i,this.z=e.z+(t.z-e.z)*i,this}cross(e){return this.crossVectors(this,e)}crossVectors(e,t){let i=e.x,r=e.y,s=e.z,o=t.x,a=t.y,l=t.z;return this.x=r*l-s*a,this.y=s*o-i*l,this.z=i*a-r*o,this}projectOnVector(e){let t=e.lengthSq();if(t===0)return this.set(0,0,0);let i=e.dot(this)/t;return this.copy(e).multiplyScalar(i)}projectOnPlane(e){return jl.copy(this).projectOnVector(e),this.sub(jl)}reflect(e){return this.sub(jl.copy(e).multiplyScalar(2*this.dot(e)))}angleTo(e){let t=Math.sqrt(this.lengthSq()*e.lengthSq());if(t===0)return Math.PI/2;let i=this.dot(e)/t;return Math.acos(lt(i,-1,1))}distanceTo(e){return Math.sqrt(this.distanceToSquared(e))}distanceToSquared(e){let t=this.x-e.x,i=this.y-e.y,r=this.z-e.z;return t*t+i*i+r*r}manhattanDistanceTo(e){return Math.abs(this.x-e.x)+Math.abs(this.y-e.y)+Math.abs(this.z-e.z)}setFromSpherical(e){return this.setFromSphericalCoords(e.radius,e.phi,e.theta)}setFromSphericalCoords(e,t,i){let r=Math.sin(t)*e;return this.x=r*Math.sin(i),this.y=Math.cos(t)*e,this.z=r*Math.cos(i),this}setFromCylindrical(e){return this.setFromCylindricalCoords(e.radius,e.theta,e.y)}setFromCylindricalCoords(e,t,i){return this.x=e*Math.sin(t),this.y=i,this.z=e*Math.cos(t),this}setFromMatrixPosition(e){let t=e.elements;return this.x=t[12],this.y=t[13],this.z=t[14],this}setFromMatrixScale(e){let t=this.setFromMatrixColumn(e,0).length(),i=this.setFromMatrixColumn(e,1).length(),r=this.setFromMatrixColumn(e,2).length();return this.x=t,this.y=i,this.z=r,this}setFromMatrixColumn(e,t){return this.fromArray(e.elements,t*4)}setFromMatrix3Column(e,t){return this.fromArray(e.elements,t*3)}setFromEuler(e){return this.x=e._x,this.y=e._y,this.z=e._z,this}setFromColor(e){return this.x=e.r,this.y=e.g,this.z=e.b,this}equals(e){return e.x===this.x&&e.y===this.y&&e.z===this.z}fromArray(e,t=0){return this.x=e[t],this.y=e[t+1],this.z=e[t+2],this}toArray(e=[],t=0){return e[t]=this.x,e[t+1]=this.y,e[t+2]=this.z,e}fromBufferAttribute(e,t){return this.x=e.getX(t),this.y=e.getY(t),this.z=e.getZ(t),this}random(){return this.x=Math.random(),this.y=Math.random(),this.z=Math.random(),this}randomDirection(){let e=Math.random()*Math.PI*2,t=Math.random()*2-1,i=Math.sqrt(1-t*t);return this.x=i*Math.cos(e),this.y=t,this.z=i*Math.sin(e),this}*[Symbol.iterator](){yield this.x,yield this.y,yield this.z}},jl=new G,nh=new un,je=class n{static{n.prototype.isMatrix3=!0}constructor(e,t,i,r,s,o,a,l,c){this.elements=[1,0,0,0,1,0,0,0,1],e!==void 0&&this.set(e,t,i,r,s,o,a,l,c)}set(e,t,i,r,s,o,a,l,c){let h=this.elements;return h[0]=e,h[1]=r,h[2]=a,h[3]=t,h[4]=s,h[5]=l,h[6]=i,h[7]=o,h[8]=c,this}identity(){return this.set(1,0,0,0,1,0,0,0,1),this}copy(e){let t=this.elements,i=e.elements;return t[0]=i[0],t[1]=i[1],t[2]=i[2],t[3]=i[3],t[4]=i[4],t[5]=i[5],t[6]=i[6],t[7]=i[7],t[8]=i[8],this}extractBasis(e,t,i){return e.setFromMatrix3Column(this,0),t.setFromMatrix3Column(this,1),i.setFromMatrix3Column(this,2),this}setFromMatrix4(e){let t=e.elements;return this.set(t[0],t[4],t[8],t[1],t[5],t[9],t[2],t[6],t[10]),this}multiply(e){return this.multiplyMatrices(this,e)}premultiply(e){return this.multiplyMatrices(e,this)}multiplyMatrices(e,t){let i=e.elements,r=t.elements,s=this.elements,o=i[0],a=i[3],l=i[6],c=i[1],h=i[4],d=i[7],u=i[2],p=i[5],x=i[8],v=r[0],m=r[3],f=r[6],b=r[1],P=r[4],w=r[7],T=r[2],E=r[5],D=r[8];return s[0]=o*v+a*b+l*T,s[3]=o*m+a*P+l*E,s[6]=o*f+a*w+l*D,s[1]=c*v+h*b+d*T,s[4]=c*m+h*P+d*E,s[7]=c*f+h*w+d*D,s[2]=u*v+p*b+x*T,s[5]=u*m+p*P+x*E,s[8]=u*f+p*w+x*D,this}multiplyScalar(e){let t=this.elements;return t[0]*=e,t[3]*=e,t[6]*=e,t[1]*=e,t[4]*=e,t[7]*=e,t[2]*=e,t[5]*=e,t[8]*=e,this}determinant(){let e=this.elements,t=e[0],i=e[1],r=e[2],s=e[3],o=e[4],a=e[5],l=e[6],c=e[7],h=e[8];return t*o*h-t*a*c-i*s*h+i*a*l+r*s*c-r*o*l}invert(){let e=this.elements,t=e[0],i=e[1],r=e[2],s=e[3],o=e[4],a=e[5],l=e[6],c=e[7],h=e[8],d=h*o-a*c,u=a*l-h*s,p=c*s-o*l,x=t*d+i*u+r*p;if(x===0)return this.set(0,0,0,0,0,0,0,0,0);let v=1/x;return e[0]=d*v,e[1]=(r*c-h*i)*v,e[2]=(a*i-r*o)*v,e[3]=u*v,e[4]=(h*t-r*l)*v,e[5]=(r*s-a*t)*v,e[6]=p*v,e[7]=(i*l-c*t)*v,e[8]=(o*t-i*s)*v,this}transpose(){let e,t=this.elements;return e=t[1],t[1]=t[3],t[3]=e,e=t[2],t[2]=t[6],t[6]=e,e=t[5],t[5]=t[7],t[7]=e,this}getNormalMatrix(e){return this.setFromMatrix4(e).invert().transpose()}transposeIntoArray(e){let t=this.elements;return e[0]=t[0],e[1]=t[3],e[2]=t[6],e[3]=t[1],e[4]=t[4],e[5]=t[7],e[6]=t[2],e[7]=t[5],e[8]=t[8],this}setUvTransform(e,t,i,r,s,o,a){let l=Math.cos(s),c=Math.sin(s);return this.set(i*l,i*c,-i*(l*o+c*a)+o+e,-r*c,r*l,-r*(-c*o+l*a)+a+t,0,0,1),this}scale(e,t){return mr("Matrix3: .scale() is deprecated. Use .makeScale() instead."),this.premultiply(Ql.makeScale(e,t)),this}rotate(e){return mr("Matrix3: .rotate() is deprecated. Use .makeRotation() instead."),this.premultiply(Ql.makeRotation(-e)),this}translate(e,t){return mr("Matrix3: .translate() is deprecated. Use .makeTranslation() instead."),this.premultiply(Ql.makeTranslation(e,t)),this}makeTranslation(e,t){return e.isVector2?this.set(1,0,e.x,0,1,e.y,0,0,1):this.set(1,0,e,0,1,t,0,0,1),this}makeRotation(e){let t=Math.cos(e),i=Math.sin(e);return this.set(t,-i,0,i,t,0,0,0,1),this}makeScale(e,t){return this.set(e,0,0,0,t,0,0,0,1),this}equals(e){let t=this.elements,i=e.elements;for(let r=0;r<9;r++)if(t[r]!==i[r])return!1;return!0}fromArray(e,t=0){for(let i=0;i<9;i++)this.elements[i]=e[i+t];return this}toArray(e=[],t=0){let i=this.elements;return e[t]=i[0],e[t+1]=i[1],e[t+2]=i[2],e[t+3]=i[3],e[t+4]=i[4],e[t+5]=i[5],e[t+6]=i[6],e[t+7]=i[7],e[t+8]=i[8],e}clone(){return new this.constructor().fromArray(this.elements)}},Ql=new je,ih=new je().set(.4123908,.3575843,.1804808,.212639,.7151687,.0721923,.0193308,.1191948,.9505322),rh=new je().set(3.2409699,-1.5373832,-.4986108,-.9692436,1.8759675,.0415551,.0556301,-.203977,1.0569715);function Hp(){let n={enabled:!0,workingColorSpace:Os,spaces:{},convert:function(r,s,o){return this.enabled===!1||s===o||!s||!o||(this.spaces[s].transfer===St&&(r.r=Ci(r.r),r.g=Ci(r.g),r.b=Ci(r.b)),this.spaces[s].primaries!==this.spaces[o].primaries&&(r.applyMatrix3(this.spaces[s].toXYZ),r.applyMatrix3(this.spaces[o].fromXYZ)),this.spaces[o].transfer===St&&(r.r=jr(r.r),r.g=jr(r.g),r.b=jr(r.b))),r},workingToColorSpace:function(r,s){return this.convert(r,this.workingColorSpace,s)},colorSpaceToWorking:function(r,s){return this.convert(r,s,this.workingColorSpace)},getPrimaries:function(r){return this.spaces[r].primaries},getTransfer:function(r){return r===Di?Bs:this.spaces[r].transfer},getToneMappingMode:function(r){return this.spaces[r].outputColorSpaceConfig.toneMappingMode||"standard"},getLuminanceCoefficients:function(r,s=this.workingColorSpace){return r.fromArray(this.spaces[s].luminanceCoefficients)},define:function(r){Object.assign(this.spaces,r)},_getMatrix:function(r,s,o){return r.copy(this.spaces[s].toXYZ).multiply(this.spaces[o].fromXYZ)},_getDrawingBufferColorSpace:function(r){return this.spaces[r].outputColorSpaceConfig.drawingBufferColorSpace},_getUnpackColorSpace:function(r=this.workingColorSpace){return this.spaces[r].workingColorSpaceConfig.unpackColorSpace},fromWorkingColorSpace:function(r,s){return mr("ColorManagement: .fromWorkingColorSpace() has been renamed to .workingToColorSpace()."),n.workingToColorSpace(r,s)},toWorkingColorSpace:function(r,s){return mr("ColorManagement: .toWorkingColorSpace() has been renamed to .colorSpaceToWorking()."),n.colorSpaceToWorking(r,s)}},e=[.64,.33,.3,.6,.15,.06],t=[.2126,.7152,.0722],i=[.3127,.329];return n.define({[Os]:{primaries:e,whitePoint:i,transfer:Bs,toXYZ:ih,fromXYZ:rh,luminanceCoefficients:t,workingColorSpaceConfig:{unpackColorSpace:Pt},outputColorSpaceConfig:{drawingBufferColorSpace:Pt}},[Pt]:{primaries:e,whitePoint:i,transfer:St,toXYZ:ih,fromXYZ:rh,luminanceCoefficients:t,outputColorSpaceConfig:{drawingBufferColorSpace:Pt}}}),n}var ht=Hp();function Ci(n){return n<.04045?n*.0773993808:Math.pow(n*.9478672986+.0521327014,2.4)}function jr(n){return n<.0031308?n*12.92:1.055*Math.pow(n,.41666)-.055}var Fr,da=class{static getDataURL(e,t="image/png"){if(/^data:/i.test(e.src)||typeof HTMLCanvasElement>"u")return e.src;let i;if(e instanceof HTMLCanvasElement)i=e;else{Fr===void 0&&(Fr=Hs("canvas")),Fr.width=e.width,Fr.height=e.height;let r=Fr.getContext("2d");e instanceof ImageData?r.putImageData(e,0,0):r.drawImage(e,0,0,e.width,e.height),i=Fr}return i.toDataURL(t)}static sRGBToLinear(e){if(typeof HTMLImageElement<"u"&&e instanceof HTMLImageElement||typeof HTMLCanvasElement<"u"&&e instanceof HTMLCanvasElement||typeof ImageBitmap<"u"&&e instanceof ImageBitmap){let t=Hs("canvas");t.width=e.width,t.height=e.height;let i=t.getContext("2d");i.drawImage(e,0,0,e.width,e.height);let r=i.getImageData(0,0,e.width,e.height),s=r.data;for(let o=0;o<s.length;o++)s[o]=Ci(s[o]/255)*255;return i.putImageData(r,0,0),t}else if(e.data){let t=e.data.slice(0);for(let i=0;i<t.length;i++)t instanceof Uint8Array||t instanceof Uint8ClampedArray?t[i]=Math.floor(Ci(t[i]/255)*255):t[i]=Ci(t[i]);return{data:t,width:e.width,height:e.height}}else return Ye("ImageUtils.sRGBToLinear(): Unsupported image type. No color space conversion applied."),e}},zp=0,is=class{constructor(e=null){this.isTextureSource=!0,Object.defineProperty(this,"id",{value:zp++}),this.uuid=ms(),this.data=e,this.dataReady=!0,this.version=0}getSize(e){let t=this.data;return typeof HTMLVideoElement<"u"&&t instanceof HTMLVideoElement?e.set(t.videoWidth,t.videoHeight,0):typeof VideoFrame<"u"&&t instanceof VideoFrame?e.set(t.displayWidth,t.displayHeight,0):t!==null?e.set(t.width,t.height,t.depth||0):e.set(0,0,0),e}set needsUpdate(e){e===!0&&this.version++}toJSON(e){let t=e===void 0||typeof e=="string";if(!t&&e.images[this.uuid]!==void 0)return e.images[this.uuid];let i={uuid:this.uuid,url:""},r=this.data;if(r!==null){let s;if(Array.isArray(r)){s=[];for(let o=0,a=r.length;o<a;o++)r[o].isDataTexture?s.push(ec(r[o].image)):s.push(ec(r[o]))}else s=ec(r);i.url=s}return t||(e.images[this.uuid]=i),i}};function ec(n){return typeof HTMLImageElement<"u"&&n instanceof HTMLImageElement||typeof HTMLCanvasElement<"u"&&n instanceof HTMLCanvasElement||typeof ImageBitmap<"u"&&n instanceof ImageBitmap?da.getDataURL(n):n.data?{data:Array.from(n.data),width:n.width,height:n.height,type:n.data.constructor.name}:(Ye("Texture: Unable to serialize Texture."),{})}var Gp=0,tc=new G,Pn=class n extends li{constructor(e=n.DEFAULT_IMAGE,t=n.DEFAULT_MAPPING,i=xi,r=xi,s=vn,o=ir,a=Bn,l=On,c=n.DEFAULT_ANISOTROPY,h=Di){super(),this.isTexture=!0,Object.defineProperty(this,"id",{value:Gp++}),this.uuid=ms(),this.name="",this.source=new is(e),this.mipmaps=[],this.mapping=t,this.channel=0,this.wrapS=i,this.wrapT=r,this.magFilter=s,this.minFilter=o,this.anisotropy=c,this.format=a,this.internalFormat=null,this.type=l,this.offset=new Xe(0,0),this.repeat=new Xe(1,1),this.center=new Xe(0,0),this.rotation=0,this.matrixAutoUpdate=!0,this.matrix=new je,this.generateMipmaps=!0,this.premultiplyAlpha=!1,this.flipY=!0,this.unpackAlignment=4,this.colorSpace=h,this.userData={},this.updateRanges=[],this.version=0,this.onUpdate=null,this.renderTarget=null,this.isRenderTargetTexture=!1,this.isArrayTexture=!!(e&&e.depth&&e.depth>1),this.pmremVersion=0,this.normalized=!1}get width(){return this.source.getSize(tc).x}get height(){return this.source.getSize(tc).y}get depth(){return this.source.getSize(tc).z}get image(){return this.source.data}set image(e){this.source.data=e}updateMatrix(){this.matrix.setUvTransform(this.offset.x,this.offset.y,this.repeat.x,this.repeat.y,this.rotation,this.center.x,this.center.y)}addUpdateRange(e,t){this.updateRanges.push({start:e,count:t})}clearUpdateRanges(){this.updateRanges.length=0}clone(){return new this.constructor().copy(this)}copy(e){return this.name=e.name,this.source=e.source,this.mipmaps=e.mipmaps.slice(0),this.mapping=e.mapping,this.channel=e.channel,this.wrapS=e.wrapS,this.wrapT=e.wrapT,this.magFilter=e.magFilter,this.minFilter=e.minFilter,this.anisotropy=e.anisotropy,this.format=e.format,this.internalFormat=e.internalFormat,this.type=e.type,this.normalized=e.normalized,this.offset.copy(e.offset),this.repeat.copy(e.repeat),this.center.copy(e.center),this.rotation=e.rotation,this.matrixAutoUpdate=e.matrixAutoUpdate,this.matrix.copy(e.matrix),this.generateMipmaps=e.generateMipmaps,this.premultiplyAlpha=e.premultiplyAlpha,this.flipY=e.flipY,this.unpackAlignment=e.unpackAlignment,this.colorSpace=e.colorSpace,this.renderTarget=e.renderTarget,this.isRenderTargetTexture=e.isRenderTargetTexture,this.isArrayTexture=e.isArrayTexture,this.userData=JSON.parse(JSON.stringify(e.userData)),this.needsUpdate=!0,this}setValues(e){for(let t in e){let i=e[t];if(i===void 0){Ye(`Texture.setValues(): parameter '${t}' has value of undefined.`);continue}let r=this[t];if(r===void 0){Ye(`Texture.setValues(): property '${t}' does not exist.`);continue}r&&i&&r.isVector2&&i.isVector2||r&&i&&r.isVector3&&i.isVector3||r&&i&&r.isMatrix3&&i.isMatrix3?r.copy(i):this[t]=i}}toJSON(e){let t=e===void 0||typeof e=="string";if(!t&&e.textures[this.uuid]!==void 0)return e.textures[this.uuid];let i={metadata:{version:4.7,type:"Texture",generator:"Texture.toJSON"},uuid:this.uuid,name:this.name,image:this.source.toJSON(e).uuid,mapping:this.mapping,channel:this.channel,repeat:[this.repeat.x,this.repeat.y],offset:[this.offset.x,this.offset.y],center:[this.center.x,this.center.y],rotation:this.rotation,wrap:[this.wrapS,this.wrapT],format:this.format,internalFormat:this.internalFormat,type:this.type,normalized:this.normalized,colorSpace:this.colorSpace,minFilter:this.minFilter,magFilter:this.magFilter,anisotropy:this.anisotropy,flipY:this.flipY,generateMipmaps:this.generateMipmaps,premultiplyAlpha:this.premultiplyAlpha,unpackAlignment:this.unpackAlignment};return Object.keys(this.userData).length>0&&(i.userData=this.userData),t||(e.textures[this.uuid]=i),i}dispose(){this.dispatchEvent({type:"dispose"})}transformUv(e){if(this.mapping!==Gc)return e;if(e.applyMatrix3(this.matrix),e.x<0||e.x>1)switch(this.wrapS){case gr:e.x=e.x-Math.floor(e.x);break;case xi:e.x=e.x<0?0:1;break;case ua:Math.abs(Math.floor(e.x)%2)===1?e.x=Math.ceil(e.x)-e.x:e.x=e.x-Math.floor(e.x);break}if(e.y<0||e.y>1)switch(this.wrapT){case gr:e.y=e.y-Math.floor(e.y);break;case xi:e.y=e.y<0?0:1;break;case ua:Math.abs(Math.floor(e.y)%2)===1?e.y=Math.ceil(e.y)-e.y:e.y=e.y-Math.floor(e.y);break}return this.flipY&&(e.y=1-e.y),e}set needsUpdate(e){e===!0&&(this.version++,this.source.needsUpdate=!0)}set needsPMREMUpdate(e){e===!0&&this.pmremVersion++}};Pn.DEFAULT_IMAGE=null;Pn.DEFAULT_MAPPING=Gc;Pn.DEFAULT_ANISOTROPY=1;var Vt=class n{static{n.prototype.isVector4=!0}constructor(e=0,t=0,i=0,r=1){this.x=e,this.y=t,this.z=i,this.w=r}get width(){return this.z}set width(e){this.z=e}get height(){return this.w}set height(e){this.w=e}set(e,t,i,r){return this.x=e,this.y=t,this.z=i,this.w=r,this}setScalar(e){return this.x=e,this.y=e,this.z=e,this.w=e,this}setX(e){return this.x=e,this}setY(e){return this.y=e,this}setZ(e){return this.z=e,this}setW(e){return this.w=e,this}setComponent(e,t){switch(e){case 0:this.x=t;break;case 1:this.y=t;break;case 2:this.z=t;break;case 3:this.w=t;break;default:throw new Error("THREE.Vector4: index is out of range: "+e)}return this}getComponent(e){switch(e){case 0:return this.x;case 1:return this.y;case 2:return this.z;case 3:return this.w;default:throw new Error("THREE.Vector4: index is out of range: "+e)}}clone(){return new this.constructor(this.x,this.y,this.z,this.w)}copy(e){return this.x=e.x,this.y=e.y,this.z=e.z,this.w=e.w!==void 0?e.w:1,this}add(e){return this.x+=e.x,this.y+=e.y,this.z+=e.z,this.w+=e.w,this}addScalar(e){return this.x+=e,this.y+=e,this.z+=e,this.w+=e,this}addVectors(e,t){return this.x=e.x+t.x,this.y=e.y+t.y,this.z=e.z+t.z,this.w=e.w+t.w,this}addScaledVector(e,t){return this.x+=e.x*t,this.y+=e.y*t,this.z+=e.z*t,this.w+=e.w*t,this}sub(e){return this.x-=e.x,this.y-=e.y,this.z-=e.z,this.w-=e.w,this}subScalar(e){return this.x-=e,this.y-=e,this.z-=e,this.w-=e,this}subVectors(e,t){return this.x=e.x-t.x,this.y=e.y-t.y,this.z=e.z-t.z,this.w=e.w-t.w,this}multiply(e){return this.x*=e.x,this.y*=e.y,this.z*=e.z,this.w*=e.w,this}multiplyScalar(e){return this.x*=e,this.y*=e,this.z*=e,this.w*=e,this}applyMatrix4(e){let t=this.x,i=this.y,r=this.z,s=this.w,o=e.elements;return this.x=o[0]*t+o[4]*i+o[8]*r+o[12]*s,this.y=o[1]*t+o[5]*i+o[9]*r+o[13]*s,this.z=o[2]*t+o[6]*i+o[10]*r+o[14]*s,this.w=o[3]*t+o[7]*i+o[11]*r+o[15]*s,this}divide(e){return this.x/=e.x,this.y/=e.y,this.z/=e.z,this.w/=e.w,this}divideScalar(e){return this.multiplyScalar(1/e)}setAxisAngleFromQuaternion(e){this.w=2*Math.acos(e.w);let t=Math.sqrt(1-e.w*e.w);return t<1e-4?(this.x=1,this.y=0,this.z=0):(this.x=e.x/t,this.y=e.y/t,this.z=e.z/t),this}setAxisAngleFromRotationMatrix(e){let t,i,r,s,l=e.elements,c=l[0],h=l[4],d=l[8],u=l[1],p=l[5],x=l[9],v=l[2],m=l[6],f=l[10];if(Math.abs(h-u)<.01&&Math.abs(d-v)<.01&&Math.abs(x-m)<.01){if(Math.abs(h+u)<.1&&Math.abs(d+v)<.1&&Math.abs(x+m)<.1&&Math.abs(c+p+f-3)<.1)return this.set(1,0,0,0),this;t=Math.PI;let P=(c+1)/2,w=(p+1)/2,T=(f+1)/2,E=(h+u)/4,D=(d+v)/4,g=(x+m)/4;return P>w&&P>T?P<.01?(i=0,r=.707106781,s=.707106781):(i=Math.sqrt(P),r=E/i,s=D/i):w>T?w<.01?(i=.707106781,r=0,s=.707106781):(r=Math.sqrt(w),i=E/r,s=g/r):T<.01?(i=.707106781,r=.707106781,s=0):(s=Math.sqrt(T),i=D/s,r=g/s),this.set(i,r,s,t),this}let b=Math.sqrt((m-x)*(m-x)+(d-v)*(d-v)+(u-h)*(u-h));return Math.abs(b)<.001&&(b=1),this.x=(m-x)/b,this.y=(d-v)/b,this.z=(u-h)/b,this.w=Math.acos((c+p+f-1)/2),this}setFromMatrixPosition(e){let t=e.elements;return this.x=t[12],this.y=t[13],this.z=t[14],this.w=t[15],this}min(e){return this.x=Math.min(this.x,e.x),this.y=Math.min(this.y,e.y),this.z=Math.min(this.z,e.z),this.w=Math.min(this.w,e.w),this}max(e){return this.x=Math.max(this.x,e.x),this.y=Math.max(this.y,e.y),this.z=Math.max(this.z,e.z),this.w=Math.max(this.w,e.w),this}clamp(e,t){return this.x=lt(this.x,e.x,t.x),this.y=lt(this.y,e.y,t.y),this.z=lt(this.z,e.z,t.z),this.w=lt(this.w,e.w,t.w),this}clampScalar(e,t){return this.x=lt(this.x,e,t),this.y=lt(this.y,e,t),this.z=lt(this.z,e,t),this.w=lt(this.w,e,t),this}clampLength(e,t){let i=this.length();return this.divideScalar(i||1).multiplyScalar(lt(i,e,t))}floor(){return this.x=Math.floor(this.x),this.y=Math.floor(this.y),this.z=Math.floor(this.z),this.w=Math.floor(this.w),this}ceil(){return this.x=Math.ceil(this.x),this.y=Math.ceil(this.y),this.z=Math.ceil(this.z),this.w=Math.ceil(this.w),this}round(){return this.x=Math.round(this.x),this.y=Math.round(this.y),this.z=Math.round(this.z),this.w=Math.round(this.w),this}roundToZero(){return this.x=Math.trunc(this.x),this.y=Math.trunc(this.y),this.z=Math.trunc(this.z),this.w=Math.trunc(this.w),this}negate(){return this.x=-this.x,this.y=-this.y,this.z=-this.z,this.w=-this.w,this}dot(e){return this.x*e.x+this.y*e.y+this.z*e.z+this.w*e.w}lengthSq(){return this.x*this.x+this.y*this.y+this.z*this.z+this.w*this.w}length(){return Math.sqrt(this.x*this.x+this.y*this.y+this.z*this.z+this.w*this.w)}manhattanLength(){return Math.abs(this.x)+Math.abs(this.y)+Math.abs(this.z)+Math.abs(this.w)}normalize(){return this.divideScalar(this.length()||1)}setLength(e){return this.normalize().multiplyScalar(e)}lerp(e,t){return this.x+=(e.x-this.x)*t,this.y+=(e.y-this.y)*t,this.z+=(e.z-this.z)*t,this.w+=(e.w-this.w)*t,this}lerpVectors(e,t,i){return this.x=e.x+(t.x-e.x)*i,this.y=e.y+(t.y-e.y)*i,this.z=e.z+(t.z-e.z)*i,this.w=e.w+(t.w-e.w)*i,this}equals(e){return e.x===this.x&&e.y===this.y&&e.z===this.z&&e.w===this.w}fromArray(e,t=0){return this.x=e[t],this.y=e[t+1],this.z=e[t+2],this.w=e[t+3],this}toArray(e=[],t=0){return e[t]=this.x,e[t+1]=this.y,e[t+2]=this.z,e[t+3]=this.w,e}fromBufferAttribute(e,t){return this.x=e.getX(t),this.y=e.getY(t),this.z=e.getZ(t),this.w=e.getW(t),this}random(){return this.x=Math.random(),this.y=Math.random(),this.z=Math.random(),this.w=Math.random(),this}*[Symbol.iterator](){yield this.x,yield this.y,yield this.z,yield this.w}},fa=class extends li{constructor(e=1,t=1,i={}){super(),i=Object.assign({generateMipmaps:!1,internalFormat:null,minFilter:vn,depthBuffer:!0,stencilBuffer:!1,resolveColorBuffer:!0,resolveDepthBuffer:!0,resolveStencilBuffer:!0,storeMultisampledColorBuffer:!0,storeMultisampledDepthBuffer:!0,storeMultisampledStencilBuffer:!0,depthTexture:null,samples:0,count:1,depth:1,multiview:!1,useArrayDepthTexture:!1},i),this.isRenderTarget=!0,this.width=e,this.height=t,this.depth=i.depth,this.scissor=new Vt(0,0,e,t),this.scissorTest=!1,this.viewport=new Vt(0,0,e,t),this.textures=[];let r={width:e,height:t,depth:i.depth},s=new Pn(r),o=i.count;for(let a=0;a<o;a++)this.textures[a]=s.clone(),this.textures[a].isRenderTargetTexture=!0,this.textures[a].renderTarget=this;this._setTextureOptions(i),this.depthBuffer=i.depthBuffer,this.stencilBuffer=i.stencilBuffer,this.resolveColorBuffer=i.resolveColorBuffer,this.resolveDepthBuffer=i.resolveDepthBuffer,this.resolveStencilBuffer=i.resolveStencilBuffer,this.storeMultisampledColorBuffer=i.storeMultisampledColorBuffer,this.storeMultisampledDepthBuffer=i.storeMultisampledDepthBuffer,this.storeMultisampledStencilBuffer=i.storeMultisampledStencilBuffer,this._depthTexture=null,this.depthTexture=i.depthTexture,this.samples=i.samples,this.multiview=i.multiview,this.useArrayDepthTexture=i.useArrayDepthTexture}_setTextureOptions(e={}){let t={minFilter:vn,generateMipmaps:!1,flipY:!1,internalFormat:null};e.mapping!==void 0&&(t.mapping=e.mapping),e.wrapS!==void 0&&(t.wrapS=e.wrapS),e.wrapT!==void 0&&(t.wrapT=e.wrapT),e.wrapR!==void 0&&(t.wrapR=e.wrapR),e.magFilter!==void 0&&(t.magFilter=e.magFilter),e.minFilter!==void 0&&(t.minFilter=e.minFilter),e.format!==void 0&&(t.format=e.format),e.type!==void 0&&(t.type=e.type),e.anisotropy!==void 0&&(t.anisotropy=e.anisotropy),e.colorSpace!==void 0&&(t.colorSpace=e.colorSpace),e.flipY!==void 0&&(t.flipY=e.flipY),e.generateMipmaps!==void 0&&(t.generateMipmaps=e.generateMipmaps),e.internalFormat!==void 0&&(t.internalFormat=e.internalFormat);for(let i=0;i<this.textures.length;i++)this.textures[i].setValues(t)}get texture(){return this.textures[0]}set texture(e){this.textures[0]=e}set depthTexture(e){this._depthTexture!==null&&this._depthTexture.renderTarget===this&&(this._depthTexture.renderTarget=null),e!==null&&e.renderTarget===null&&(e.renderTarget=this),this._depthTexture=e}get depthTexture(){return this._depthTexture}setSize(e,t,i=1){if(this.width!==e||this.height!==t||this.depth!==i){this.width=e,this.height=t,this.depth=i;for(let r=0,s=this.textures.length;r<s;r++)this.textures[r].image.width=e,this.textures[r].image.height=t,this.textures[r].image.depth=i,this.textures[r].isData3DTexture!==!0&&(this.textures[r].isArrayTexture=this.textures[r].image.depth>1);this.dispose()}this.viewport.set(0,0,e,t),this.scissor.set(0,0,e,t)}clone(){return new this.constructor().copy(this)}copy(e){this.width=e.width,this.height=e.height,this.depth=e.depth,this.scissor.copy(e.scissor),this.scissorTest=e.scissorTest,this.viewport.copy(e.viewport),this.textures.length=0;for(let t=0,i=e.textures.length;t<i;t++){this.textures[t]=e.textures[t].clone(),this.textures[t].isRenderTargetTexture=!0,this.textures[t].renderTarget=this;let r=Object.assign({},e.textures[t].image);this.textures[t].source=new is(r)}if(this.depthBuffer=e.depthBuffer,this.stencilBuffer=e.stencilBuffer,this.resolveColorBuffer=e.resolveColorBuffer,this.resolveDepthBuffer=e.resolveDepthBuffer,this.resolveStencilBuffer=e.resolveStencilBuffer,this.storeMultisampledColorBuffer=e.storeMultisampledColorBuffer,this.storeMultisampledDepthBuffer=e.storeMultisampledDepthBuffer,this.storeMultisampledStencilBuffer=e.storeMultisampledStencilBuffer,e.depthTexture!==null)if(e.depthTexture.renderTarget===e){let t=e.depthTexture.clone();t.renderTarget=null,this.depthTexture=t}else this.depthTexture=e.depthTexture;return this.samples=e.samples,this.multiview=e.multiview,this.useArrayDepthTexture=e.useArrayDepthTexture,this}dispose(){this.dispatchEvent({type:"dispose"})}},Dn=class extends fa{constructor(e=1,t=1,i={}){super(e,t,i),this.isWebGLRenderTarget=!0}},zs=class extends Pn{constructor(e=null,t=1,i=1,r=1){super(null),this.isDataArrayTexture=!0,this.image={data:e,width:t,height:i,depth:r},this.magFilter=en,this.minFilter=en,this.wrapR=xi,this.generateMipmaps=!1,this.flipY=!1,this.unpackAlignment=1,this.layerUpdates=new Set}copy(e){return super.copy(e),this.wrapR=e.wrapR,this}addLayerUpdate(e){this.layerUpdates.add(e)}clearLayerUpdates(){this.layerUpdates.clear()}};var pa=class extends Pn{constructor(e=null,t=1,i=1,r=1){super(null),this.isData3DTexture=!0,this.image={data:e,width:t,height:i,depth:r},this.magFilter=en,this.minFilter=en,this.wrapR=xi,this.generateMipmaps=!1,this.flipY=!1,this.unpackAlignment=1}copy(e){return super.copy(e),this.wrapR=e.wrapR,this}};var wt=class n{static{n.prototype.isMatrix4=!0}constructor(e,t,i,r,s,o,a,l,c,h,d,u,p,x,v,m){this.elements=[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],e!==void 0&&this.set(e,t,i,r,s,o,a,l,c,h,d,u,p,x,v,m)}set(e,t,i,r,s,o,a,l,c,h,d,u,p,x,v,m){let f=this.elements;return f[0]=e,f[4]=t,f[8]=i,f[12]=r,f[1]=s,f[5]=o,f[9]=a,f[13]=l,f[2]=c,f[6]=h,f[10]=d,f[14]=u,f[3]=p,f[7]=x,f[11]=v,f[15]=m,this}identity(){return this.set(1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1),this}clone(){return new n().fromArray(this.elements)}copy(e){let t=this.elements,i=e.elements;return t[0]=i[0],t[1]=i[1],t[2]=i[2],t[3]=i[3],t[4]=i[4],t[5]=i[5],t[6]=i[6],t[7]=i[7],t[8]=i[8],t[9]=i[9],t[10]=i[10],t[11]=i[11],t[12]=i[12],t[13]=i[13],t[14]=i[14],t[15]=i[15],this}copyPosition(e){let t=this.elements,i=e.elements;return t[12]=i[12],t[13]=i[13],t[14]=i[14],this}setFromMatrix3(e){let t=e.elements;return this.set(t[0],t[3],t[6],0,t[1],t[4],t[7],0,t[2],t[5],t[8],0,0,0,0,1),this}extractBasis(e,t,i){return this.determinantAffine()===0?(e.set(1,0,0),t.set(0,1,0),i.set(0,0,1),this):(e.setFromMatrixColumn(this,0),t.setFromMatrixColumn(this,1),i.setFromMatrixColumn(this,2),this)}makeBasis(e,t,i){return this.set(e.x,t.x,i.x,0,e.y,t.y,i.y,0,e.z,t.z,i.z,0,0,0,0,1),this}extractRotation(e){if(e.determinantAffine()===0)return this.identity();let t=this.elements,i=e.elements,r=1/Or.setFromMatrixColumn(e,0).length(),s=1/Or.setFromMatrixColumn(e,1).length(),o=1/Or.setFromMatrixColumn(e,2).length();return t[0]=i[0]*r,t[1]=i[1]*r,t[2]=i[2]*r,t[3]=0,t[4]=i[4]*s,t[5]=i[5]*s,t[6]=i[6]*s,t[7]=0,t[8]=i[8]*o,t[9]=i[9]*o,t[10]=i[10]*o,t[11]=0,t[12]=0,t[13]=0,t[14]=0,t[15]=1,this}makeRotationFromEuler(e){let t=this.elements,i=e.x,r=e.y,s=e.z,o=Math.cos(i),a=Math.sin(i),l=Math.cos(r),c=Math.sin(r),h=Math.cos(s),d=Math.sin(s);if(e.order==="XYZ"){let u=o*h,p=o*d,x=a*h,v=a*d;t[0]=l*h,t[4]=-l*d,t[8]=c,t[1]=p+x*c,t[5]=u-v*c,t[9]=-a*l,t[2]=v-u*c,t[6]=x+p*c,t[10]=o*l}else if(e.order==="YXZ"){let u=l*h,p=l*d,x=c*h,v=c*d;t[0]=u+v*a,t[4]=x*a-p,t[8]=o*c,t[1]=o*d,t[5]=o*h,t[9]=-a,t[2]=p*a-x,t[6]=v+u*a,t[10]=o*l}else if(e.order==="ZXY"){let u=l*h,p=l*d,x=c*h,v=c*d;t[0]=u-v*a,t[4]=-o*d,t[8]=x+p*a,t[1]=p+x*a,t[5]=o*h,t[9]=v-u*a,t[2]=-o*c,t[6]=a,t[10]=o*l}else if(e.order==="ZYX"){let u=o*h,p=o*d,x=a*h,v=a*d;t[0]=l*h,t[4]=x*c-p,t[8]=u*c+v,t[1]=l*d,t[5]=v*c+u,t[9]=p*c-x,t[2]=-c,t[6]=a*l,t[10]=o*l}else if(e.order==="YZX"){let u=o*l,p=o*c,x=a*l,v=a*c;t[0]=l*h,t[4]=v-u*d,t[8]=x*d+p,t[1]=d,t[5]=o*h,t[9]=-a*h,t[2]=-c*h,t[6]=p*d+x,t[10]=u-v*d}else if(e.order==="XZY"){let u=o*l,p=o*c,x=a*l,v=a*c;t[0]=l*h,t[4]=-d,t[8]=c*h,t[1]=u*d+v,t[5]=o*h,t[9]=p*d-x,t[2]=x*d-p,t[6]=a*h,t[10]=v*d+u}return t[3]=0,t[7]=0,t[11]=0,t[12]=0,t[13]=0,t[14]=0,t[15]=1,this}makeRotationFromQuaternion(e){return this.compose(Vp,e,Wp)}lookAt(e,t,i){let r=this.elements;return Gn.subVectors(e,t),Gn.lengthSq()===0&&(Gn.z=1),Gn.normalize(),zi.crossVectors(i,Gn),zi.lengthSq()===0&&(Math.abs(i.z)===1?Gn.x+=1e-4:Gn.z+=1e-4,Gn.normalize(),zi.crossVectors(i,Gn)),zi.normalize(),Lo.crossVectors(Gn,zi),r[0]=zi.x,r[4]=Lo.x,r[8]=Gn.x,r[1]=zi.y,r[5]=Lo.y,r[9]=Gn.y,r[2]=zi.z,r[6]=Lo.z,r[10]=Gn.z,this}multiply(e){return this.multiplyMatrices(this,e)}premultiply(e){return this.multiplyMatrices(e,this)}multiplyMatrices(e,t){let i=e.elements,r=t.elements,s=this.elements,o=i[0],a=i[4],l=i[8],c=i[12],h=i[1],d=i[5],u=i[9],p=i[13],x=i[2],v=i[6],m=i[10],f=i[14],b=i[3],P=i[7],w=i[11],T=i[15],E=r[0],D=r[4],g=r[8],S=r[12],_=r[1],A=r[5],I=r[9],k=r[13],N=r[2],U=r[6],z=r[10],Y=r[14],ee=r[3],J=r[7],ie=r[11],ce=r[15];return s[0]=o*E+a*_+l*N+c*ee,s[4]=o*D+a*A+l*U+c*J,s[8]=o*g+a*I+l*z+c*ie,s[12]=o*S+a*k+l*Y+c*ce,s[1]=h*E+d*_+u*N+p*ee,s[5]=h*D+d*A+u*U+p*J,s[9]=h*g+d*I+u*z+p*ie,s[13]=h*S+d*k+u*Y+p*ce,s[2]=x*E+v*_+m*N+f*ee,s[6]=x*D+v*A+m*U+f*J,s[10]=x*g+v*I+m*z+f*ie,s[14]=x*S+v*k+m*Y+f*ce,s[3]=b*E+P*_+w*N+T*ee,s[7]=b*D+P*A+w*U+T*J,s[11]=b*g+P*I+w*z+T*ie,s[15]=b*S+P*k+w*Y+T*ce,this}multiplyScalar(e){let t=this.elements;return t[0]*=e,t[4]*=e,t[8]*=e,t[12]*=e,t[1]*=e,t[5]*=e,t[9]*=e,t[13]*=e,t[2]*=e,t[6]*=e,t[10]*=e,t[14]*=e,t[3]*=e,t[7]*=e,t[11]*=e,t[15]*=e,this}determinant(){let e=this.elements,t=e[0],i=e[4],r=e[8],s=e[12],o=e[1],a=e[5],l=e[9],c=e[13],h=e[2],d=e[6],u=e[10],p=e[14],x=e[3],v=e[7],m=e[11],f=e[15],b=l*p-c*u,P=a*p-c*d,w=a*u-l*d,T=o*p-c*h,E=o*u-l*h,D=o*d-a*h;return t*(v*b-m*P+f*w)-i*(x*b-m*T+f*E)+r*(x*P-v*T+f*D)-s*(x*w-v*E+m*D)}determinantAffine(){let e=this.elements,t=e[0],i=e[4],r=e[8],s=e[1],o=e[5],a=e[9],l=e[2],c=e[6],h=e[10];return t*(o*h-a*c)-i*(s*h-a*l)+r*(s*c-o*l)}transpose(){let e=this.elements,t;return t=e[1],e[1]=e[4],e[4]=t,t=e[2],e[2]=e[8],e[8]=t,t=e[6],e[6]=e[9],e[9]=t,t=e[3],e[3]=e[12],e[12]=t,t=e[7],e[7]=e[13],e[13]=t,t=e[11],e[11]=e[14],e[14]=t,this}setPosition(e,t,i){let r=this.elements;return e.isVector3?(r[12]=e.x,r[13]=e.y,r[14]=e.z):(r[12]=e,r[13]=t,r[14]=i),this}invert(){let e=this.elements,t=e[0],i=e[1],r=e[2],s=e[3],o=e[4],a=e[5],l=e[6],c=e[7],h=e[8],d=e[9],u=e[10],p=e[11],x=e[12],v=e[13],m=e[14],f=e[15],b=t*a-i*o,P=t*l-r*o,w=t*c-s*o,T=i*l-r*a,E=i*c-s*a,D=r*c-s*l,g=h*v-d*x,S=h*m-u*x,_=h*f-p*x,A=d*m-u*v,I=d*f-p*v,k=u*f-p*m,N=b*k-P*I+w*A+T*_-E*S+D*g;if(N===0)return this.set(0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0);let U=1/N;return e[0]=(a*k-l*I+c*A)*U,e[1]=(r*I-i*k-s*A)*U,e[2]=(v*D-m*E+f*T)*U,e[3]=(u*E-d*D-p*T)*U,e[4]=(l*_-o*k-c*S)*U,e[5]=(t*k-r*_+s*S)*U,e[6]=(m*w-x*D-f*P)*U,e[7]=(h*D-u*w+p*P)*U,e[8]=(o*I-a*_+c*g)*U,e[9]=(i*_-t*I-s*g)*U,e[10]=(x*E-v*w+f*b)*U,e[11]=(d*w-h*E-p*b)*U,e[12]=(a*S-o*A-l*g)*U,e[13]=(t*A-i*S+r*g)*U,e[14]=(v*P-x*T-m*b)*U,e[15]=(h*T-d*P+u*b)*U,this}scale(e){let t=this.elements,i=e.x,r=e.y,s=e.z;return t[0]*=i,t[4]*=r,t[8]*=s,t[1]*=i,t[5]*=r,t[9]*=s,t[2]*=i,t[6]*=r,t[10]*=s,t[3]*=i,t[7]*=r,t[11]*=s,this}getMaxScaleOnAxis(){let e=this.elements,t=e[0]*e[0]+e[1]*e[1]+e[2]*e[2],i=e[4]*e[4]+e[5]*e[5]+e[6]*e[6],r=e[8]*e[8]+e[9]*e[9]+e[10]*e[10];return Math.sqrt(Math.max(t,i,r))}makeTranslation(e,t,i){return e.isVector3?this.set(1,0,0,e.x,0,1,0,e.y,0,0,1,e.z,0,0,0,1):this.set(1,0,0,e,0,1,0,t,0,0,1,i,0,0,0,1),this}makeRotationX(e){let t=Math.cos(e),i=Math.sin(e);return this.set(1,0,0,0,0,t,-i,0,0,i,t,0,0,0,0,1),this}makeRotationY(e){let t=Math.cos(e),i=Math.sin(e);return this.set(t,0,i,0,0,1,0,0,-i,0,t,0,0,0,0,1),this}makeRotationZ(e){let t=Math.cos(e),i=Math.sin(e);return this.set(t,-i,0,0,i,t,0,0,0,0,1,0,0,0,0,1),this}makeRotationAxis(e,t){let i=Math.cos(t),r=Math.sin(t),s=1-i,o=e.x,a=e.y,l=e.z,c=s*o,h=s*a;return this.set(c*o+i,c*a-r*l,c*l+r*a,0,c*a+r*l,h*a+i,h*l-r*o,0,c*l-r*a,h*l+r*o,s*l*l+i,0,0,0,0,1),this}makeScale(e,t,i){return this.set(e,0,0,0,0,t,0,0,0,0,i,0,0,0,0,1),this}makeShear(e,t,i,r,s,o){return this.set(1,i,s,0,e,1,o,0,t,r,1,0,0,0,0,1),this}compose(e,t,i){let r=this.elements,s=t._x,o=t._y,a=t._z,l=t._w,c=s+s,h=o+o,d=a+a,u=s*c,p=s*h,x=s*d,v=o*h,m=o*d,f=a*d,b=l*c,P=l*h,w=l*d,T=i.x,E=i.y,D=i.z;return r[0]=(1-(v+f))*T,r[1]=(p+w)*T,r[2]=(x-P)*T,r[3]=0,r[4]=(p-w)*E,r[5]=(1-(u+f))*E,r[6]=(m+b)*E,r[7]=0,r[8]=(x+P)*D,r[9]=(m-b)*D,r[10]=(1-(u+v))*D,r[11]=0,r[12]=e.x,r[13]=e.y,r[14]=e.z,r[15]=1,this}decompose(e,t,i){let r=this.elements;e.x=r[12],e.y=r[13],e.z=r[14];let s=this.determinantAffine();if(s===0)return i.set(1,1,1),t.identity(),this;let o=Or.set(r[0],r[1],r[2]).length(),a=Or.set(r[4],r[5],r[6]).length(),l=Or.set(r[8],r[9],r[10]).length();s<0&&(o=-o),ri.copy(this);let c=1/o,h=1/a,d=1/l;return ri.elements[0]*=c,ri.elements[1]*=c,ri.elements[2]*=c,ri.elements[4]*=h,ri.elements[5]*=h,ri.elements[6]*=h,ri.elements[8]*=d,ri.elements[9]*=d,ri.elements[10]*=d,t.setFromRotationMatrix(ri),i.x=o,i.y=a,i.z=l,this}makePerspective(e,t,i,r,s,o,a=ai,l=!1){let c=this.elements,h=2*s/(t-e),d=2*s/(i-r),u=(t+e)/(t-e),p=(i+r)/(i-r),x,v;if(l)x=s/(o-s),v=o*s/(o-s);else if(a===ai)x=-(o+s)/(o-s),v=-2*o*s/(o-s);else if(a===es)x=-o/(o-s),v=-o*s/(o-s);else throw new Error("THREE.Matrix4.makePerspective(): Invalid coordinate system: "+a);return c[0]=h,c[4]=0,c[8]=u,c[12]=0,c[1]=0,c[5]=d,c[9]=p,c[13]=0,c[2]=0,c[6]=0,c[10]=x,c[14]=v,c[3]=0,c[7]=0,c[11]=-1,c[15]=0,this}makeOrthographic(e,t,i,r,s,o,a=ai,l=!1){let c=this.elements,h=2/(t-e),d=2/(i-r),u=-(t+e)/(t-e),p=-(i+r)/(i-r),x,v;if(l)x=1/(o-s),v=o/(o-s);else if(a===ai)x=-2/(o-s),v=-(o+s)/(o-s);else if(a===es)x=-1/(o-s),v=-s/(o-s);else throw new Error("THREE.Matrix4.makeOrthographic(): Invalid coordinate system: "+a);return c[0]=h,c[4]=0,c[8]=0,c[12]=u,c[1]=0,c[5]=d,c[9]=0,c[13]=p,c[2]=0,c[6]=0,c[10]=x,c[14]=v,c[3]=0,c[7]=0,c[11]=0,c[15]=1,this}equals(e){let t=this.elements,i=e.elements;for(let r=0;r<16;r++)if(t[r]!==i[r])return!1;return!0}fromArray(e,t=0){for(let i=0;i<16;i++)this.elements[i]=e[i+t];return this}toArray(e=[],t=0){let i=this.elements;return e[t]=i[0],e[t+1]=i[1],e[t+2]=i[2],e[t+3]=i[3],e[t+4]=i[4],e[t+5]=i[5],e[t+6]=i[6],e[t+7]=i[7],e[t+8]=i[8],e[t+9]=i[9],e[t+10]=i[10],e[t+11]=i[11],e[t+12]=i[12],e[t+13]=i[13],e[t+14]=i[14],e[t+15]=i[15],e}},Or=new G,ri=new wt,Vp=new G(0,0,0),Wp=new G(1,1,1),zi=new G,Lo=new G,Gn=new G,sh=new wt,oh=new un,$i=class n{constructor(e=0,t=0,i=0,r=n.DEFAULT_ORDER){this.isEuler=!0,this._x=e,this._y=t,this._z=i,this._order=r}get x(){return this._x}set x(e){this._x=e,this._onChangeCallback()}get y(){return this._y}set y(e){this._y=e,this._onChangeCallback()}get z(){return this._z}set z(e){this._z=e,this._onChangeCallback()}get order(){return this._order}set order(e){this._order=e,this._onChangeCallback()}set(e,t,i,r=this._order){return this._x=e,this._y=t,this._z=i,this._order=r,this._onChangeCallback(),this}clone(){return new this.constructor(this._x,this._y,this._z,this._order)}copy(e){return this._x=e._x,this._y=e._y,this._z=e._z,this._order=e._order,this._onChangeCallback(),this}setFromRotationMatrix(e,t=this._order,i=!0){let r=e.elements,s=r[0],o=r[4],a=r[8],l=r[1],c=r[5],h=r[9],d=r[2],u=r[6],p=r[10];switch(t){case"XYZ":this._y=Math.asin(lt(a,-1,1)),Math.abs(a)<.9999999?(this._x=Math.atan2(-h,p),this._z=Math.atan2(-o,s)):(this._x=Math.atan2(u,c),this._z=0);break;case"YXZ":this._x=Math.asin(-lt(h,-1,1)),Math.abs(h)<.9999999?(this._y=Math.atan2(a,p),this._z=Math.atan2(l,c)):(this._y=Math.atan2(-d,s),this._z=0);break;case"ZXY":this._x=Math.asin(lt(u,-1,1)),Math.abs(u)<.9999999?(this._y=Math.atan2(-d,p),this._z=Math.atan2(-o,c)):(this._y=0,this._z=Math.atan2(l,s));break;case"ZYX":this._y=Math.asin(-lt(d,-1,1)),Math.abs(d)<.9999999?(this._x=Math.atan2(u,p),this._z=Math.atan2(l,s)):(this._x=0,this._z=Math.atan2(-o,c));break;case"YZX":this._z=Math.asin(lt(l,-1,1)),Math.abs(l)<.9999999?(this._x=Math.atan2(-h,c),this._y=Math.atan2(-d,s)):(this._x=0,this._y=Math.atan2(a,p));break;case"XZY":this._z=Math.asin(-lt(o,-1,1)),Math.abs(o)<.9999999?(this._x=Math.atan2(u,c),this._y=Math.atan2(a,s)):(this._x=Math.atan2(-h,p),this._y=0);break;default:Ye("Euler: .setFromRotationMatrix() encountered an unknown order: "+t)}return this._order=t,i===!0&&this._onChangeCallback(),this}setFromQuaternion(e,t,i){return sh.makeRotationFromQuaternion(e),this.setFromRotationMatrix(sh,t,i)}setFromVector3(e,t=this._order){return this.set(e.x,e.y,e.z,t)}reorder(e){return oh.setFromEuler(this),this.setFromQuaternion(oh,e)}equals(e){return e._x===this._x&&e._y===this._y&&e._z===this._z&&e._order===this._order}fromArray(e){return this._x=e[0],this._y=e[1],this._z=e[2],e[3]!==void 0&&(this._order=e[3]),this._onChangeCallback(),this}toArray(e=[],t=0){return e[t]=this._x,e[t+1]=this._y,e[t+2]=this._z,e[t+3]=this._order,e}_onChange(e){return this._onChangeCallback=e,this}_onChangeCallback(){}*[Symbol.iterator](){yield this._x,yield this._y,yield this._z,yield this._order}};$i.DEFAULT_ORDER="XYZ";var rs=class{constructor(){this.mask=1}set(e){this.mask=(1<<e|0)>>>0}enable(e){this.mask|=1<<e|0}enableAll(){this.mask=-1}toggle(e){this.mask^=1<<e|0}disable(e){this.mask&=~(1<<e|0)}disableAll(){this.mask=0}test(e){return(this.mask&e.mask)!==0}isEnabled(e){return(this.mask&(1<<e|0))!==0}},Xp=0,ah=new G,Br=new un,wi=new wt,Do=new G,Cs=new G,qp=new G,$p=new un,lh=new G(1,0,0),ch=new G(0,1,0),uh=new G(0,0,1),hh={type:"added"},Yp={type:"removed"},Hr={type:"childadded",child:null},nc={type:"childremoved",child:null},yn=class n extends li{constructor(){super(),this.isObject3D=!0,Object.defineProperty(this,"id",{value:Xp++}),this.uuid=ms(),this.name="",this.type="Object3D",this.parent=null,this.children=[],this.up=n.DEFAULT_UP.clone();let e=new G,t=new $i,i=new un,r=new G(1,1,1);function s(){i.setFromEuler(t,!1)}function o(){t.setFromQuaternion(i,void 0,!1)}t._onChange(s),i._onChange(o),Object.defineProperties(this,{position:{configurable:!0,enumerable:!0,value:e},rotation:{configurable:!0,enumerable:!0,value:t},quaternion:{configurable:!0,enumerable:!0,value:i},scale:{configurable:!0,enumerable:!0,value:r},modelViewMatrix:{value:new wt},normalMatrix:{value:new je}}),this.matrix=new wt,this.matrixWorld=new wt,this.matrixAutoUpdate=n.DEFAULT_MATRIX_AUTO_UPDATE,this.matrixWorldAutoUpdate=n.DEFAULT_MATRIX_WORLD_AUTO_UPDATE,this.matrixWorldNeedsUpdate=!1,this.layers=new rs,this.visible=!0,this.castShadow=!1,this.receiveShadow=!1,this.frustumCulled=!0,this.renderOrder=0,this.animations=[],this.customDepthMaterial=void 0,this.customDistanceMaterial=void 0,this.static=!1,this.userData={},this.pivot=null}onBeforeShadow(){}onAfterShadow(){}onBeforeRender(){}onAfterRender(){}applyMatrix4(e){this.matrixAutoUpdate&&this.updateMatrix(),this.matrix.premultiply(e),this.matrix.decompose(this.position,this.quaternion,this.scale)}applyQuaternion(e){return this.quaternion.premultiply(e),this}setRotationFromAxisAngle(e,t){this.quaternion.setFromAxisAngle(e,t)}setRotationFromEuler(e){this.quaternion.setFromEuler(e,!0)}setRotationFromMatrix(e){this.quaternion.setFromRotationMatrix(e)}setRotationFromQuaternion(e){this.quaternion.copy(e)}rotateOnAxis(e,t){return Br.setFromAxisAngle(e,t),this.quaternion.multiply(Br),this}rotateOnWorldAxis(e,t){return Br.setFromAxisAngle(e,t),this.quaternion.premultiply(Br),this}rotateX(e){return this.rotateOnAxis(lh,e)}rotateY(e){return this.rotateOnAxis(ch,e)}rotateZ(e){return this.rotateOnAxis(uh,e)}translateOnAxis(e,t){return ah.copy(e).applyQuaternion(this.quaternion),this.position.add(ah.multiplyScalar(t)),this}translateX(e){return this.translateOnAxis(lh,e)}translateY(e){return this.translateOnAxis(ch,e)}translateZ(e){return this.translateOnAxis(uh,e)}localToWorld(e){return this.updateWorldMatrix(!0,!1),e.applyMatrix4(this.matrixWorld)}worldToLocal(e){return this.updateWorldMatrix(!0,!1),e.applyMatrix4(wi.copy(this.matrixWorld).invert())}lookAt(e,t,i){e.isVector3?Do.copy(e):Do.set(e,t,i);let r=this.parent;this.updateWorldMatrix(!0,!1),Cs.setFromMatrixPosition(this.matrixWorld),this.isCamera||this.isLight?wi.lookAt(Cs,Do,this.up):wi.lookAt(Do,Cs,this.up),this.quaternion.setFromRotationMatrix(wi),r&&(wi.extractRotation(r.matrixWorld),Br.setFromRotationMatrix(wi),this.quaternion.premultiply(Br.invert()))}add(e){if(arguments.length>1){for(let t=0;t<arguments.length;t++)this.add(arguments[t]);return this}return e===this?(Ke("Object3D.add: object can't be added as a child of itself.",e),this):(e&&e.isObject3D?(e.removeFromParent(),e.parent=this,this.children.push(e),e.dispatchEvent(hh),Hr.child=e,this.dispatchEvent(Hr),Hr.child=null):Ke("Object3D.add: object not an instance of THREE.Object3D.",e),this)}remove(e){if(arguments.length>1){for(let i=0;i<arguments.length;i++)this.remove(arguments[i]);return this}let t=this.children.indexOf(e);return t!==-1&&(e.parent=null,this.children.splice(t,1),e.dispatchEvent(Yp),nc.child=e,this.dispatchEvent(nc),nc.child=null),this}removeFromParent(){let e=this.parent;return e!==null&&e.remove(this),this}clear(){return this.remove(...this.children)}attach(e){return this.updateWorldMatrix(!0,!1),wi.copy(this.matrixWorld).invert(),e.parent!==null&&(e.parent.updateWorldMatrix(!0,!1),wi.multiply(e.parent.matrixWorld)),e.applyMatrix4(wi),e.removeFromParent(),e.parent=this,this.children.push(e),e.updateWorldMatrix(!1,!0),e.dispatchEvent(hh),Hr.child=e,this.dispatchEvent(Hr),Hr.child=null,this}getObjectById(e){return this.getObjectByProperty("id",e)}getObjectByName(e){return this.getObjectByProperty("name",e)}getObjectByProperty(e,t){if(this[e]===t)return this;for(let i=0,r=this.children.length;i<r;i++){let o=this.children[i].getObjectByProperty(e,t);if(o!==void 0)return o}}getObjectsByProperty(e,t,i=[]){this[e]===t&&i.push(this);let r=this.children;for(let s=0,o=r.length;s<o;s++)r[s].getObjectsByProperty(e,t,i);return i}getWorldPosition(e){return this.updateWorldMatrix(!0,!1),e.setFromMatrixPosition(this.matrixWorld)}getWorldQuaternion(e){return this.updateWorldMatrix(!0,!1),this.matrixWorld.decompose(Cs,e,qp),e}getWorldScale(e){return this.updateWorldMatrix(!0,!1),this.matrixWorld.decompose(Cs,$p,e),e}getWorldDirection(e){this.updateWorldMatrix(!0,!1);let t=this.matrixWorld.elements;return e.set(t[8],t[9],t[10]).normalize()}raycast(){}intersectsFrustum(){}traverse(e){e(this);let t=this.children;for(let i=0,r=t.length;i<r;i++)t[i].traverse(e)}traverseVisible(e){if(this.visible===!1)return;e(this);let t=this.children;for(let i=0,r=t.length;i<r;i++)t[i].traverseVisible(e)}traverseAncestors(e){let t=this.parent;t!==null&&(e(t),t.traverseAncestors(e))}updateMatrix(){this.matrix.compose(this.position,this.quaternion,this.scale);let e=this.pivot;if(e!==null){let t=e.x,i=e.y,r=e.z,s=this.matrix.elements;s[12]+=t-s[0]*t-s[4]*i-s[8]*r,s[13]+=i-s[1]*t-s[5]*i-s[9]*r,s[14]+=r-s[2]*t-s[6]*i-s[10]*r}this.matrixWorldNeedsUpdate=!0}updateMatrixWorld(e){this.matrixAutoUpdate&&this.updateMatrix(),(this.matrixWorldNeedsUpdate||e)&&(this.matrixWorldAutoUpdate===!0&&(this.parent===null?this.matrixWorld.copy(this.matrix):this.matrixWorld.multiplyMatrices(this.parent.matrixWorld,this.matrix)),this.matrixWorldNeedsUpdate=!1,e=!0);let t=this.children;for(let i=0,r=t.length;i<r;i++)t[i].updateMatrixWorld(e)}updateWorldMatrix(e,t,i=!1){let r=this.parent;if(e===!0&&r!==null&&r.updateWorldMatrix(!0,!1),this.matrixAutoUpdate&&this.updateMatrix(),(this.matrixWorldNeedsUpdate||i)&&(this.matrixWorldAutoUpdate===!0&&(this.parent===null?this.matrixWorld.copy(this.matrix):this.matrixWorld.multiplyMatrices(this.parent.matrixWorld,this.matrix)),this.matrixWorldNeedsUpdate=!1,i=!0),t===!0){let s=this.children;for(let o=0,a=s.length;o<a;o++)s[o].updateWorldMatrix(!1,!0,i)}}toJSON(e){let t=e===void 0||typeof e=="string",i={};t&&(e={geometries:{},materials:{},textures:{},images:{},shapes:{},skeletons:{},animations:{},nodes:{}},i.metadata={version:4.7,type:"Object",generator:"Object3D.toJSON"});let r={};r.uuid=this.uuid,r.type=this.type,r.name=this.name,r.castShadow=this.castShadow,r.receiveShadow=this.receiveShadow,r.visible=this.visible,r.frustumCulled=this.frustumCulled,r.renderOrder=this.renderOrder,r.static=this.static,r.matrixAutoUpdate=this.matrixAutoUpdate,Object.keys(this.userData).length>0&&(r.userData=this.userData),r.layers=this.layers.mask,r.matrix=this.matrix.toArray(),r.up=this.up.toArray(),this.pivot!==null&&(r.pivot=this.pivot.toArray()),this.morphTargetDictionary!==void 0&&(r.morphTargetDictionary=Object.assign({},this.morphTargetDictionary)),this.morphTargetInfluences!==void 0&&(r.morphTargetInfluences=this.morphTargetInfluences.slice()),this.isInstancedMesh&&(r.type="InstancedMesh",r.count=this.count,r.instanceMatrix=this.instanceMatrix.toJSON(),this.instanceColor!==null&&(r.instanceColor=this.instanceColor.toJSON())),this.isBatchedMesh&&(r.type="BatchedMesh",r.perObjectFrustumCulled=this.perObjectFrustumCulled,r.sortObjects=this.sortObjects,r.drawRanges=this._drawRanges,r.reservedRanges=this._reservedRanges,r.geometryInfo=this._geometryInfo.map(a=>({...a,boundingBox:a.boundingBox?a.boundingBox.toJSON():void 0,boundingSphere:a.boundingSphere?a.boundingSphere.toJSON():void 0})),r.instanceInfo=this._instanceInfo.map(a=>({...a})),r.availableInstanceIds=this._availableInstanceIds.slice(),r.availableGeometryIds=this._availableGeometryIds.slice(),r.nextIndexStart=this._nextIndexStart,r.nextVertexStart=this._nextVertexStart,r.geometryCount=this._geometryCount,r.maxInstanceCount=this._maxInstanceCount,r.maxVertexCount=this._maxVertexCount,r.maxIndexCount=this._maxIndexCount,r.geometryInitialized=this._geometryInitialized,r.matricesTexture=this._matricesTexture.toJSON(e),r.indirectTexture=this._indirectTexture.toJSON(e),this._colorsTexture!==null&&(r.colorsTexture=this._colorsTexture.toJSON(e)),this.boundingSphere!==null&&(r.boundingSphere=this.boundingSphere.toJSON()),this.boundingBox!==null&&(r.boundingBox=this.boundingBox.toJSON()));function s(a,l){return a[l.uuid]===void 0&&(a[l.uuid]=l.toJSON(e)),l.uuid}if(this.isScene)this.background&&(this.background.isColor?r.background=this.background.toJSON():this.background.isTexture&&(r.background=this.background.toJSON(e).uuid)),this.environment&&this.environment.isTexture&&this.environment.isRenderTargetTexture!==!0&&(r.environment=this.environment.toJSON(e).uuid);else if(this.isMesh||this.isLine||this.isPoints){r.geometry=s(e.geometries,this.geometry);let a=this.geometry.parameters;if(a!==void 0&&a.shapes!==void 0){let l=a.shapes;if(Array.isArray(l))for(let c=0,h=l.length;c<h;c++){let d=l[c];s(e.shapes,d)}else s(e.shapes,l)}}if(this.isSkinnedMesh&&(r.bindMode=this.bindMode,r.bindMatrix=this.bindMatrix.toArray(),this.skeleton!==void 0&&(s(e.skeletons,this.skeleton),r.skeleton=this.skeleton.uuid)),this.material!==void 0)if(Array.isArray(this.material)){let a=[];for(let l=0,c=this.material.length;l<c;l++)a.push(s(e.materials,this.material[l]));r.material=a}else r.material=s(e.materials,this.material);if(this.children.length>0){r.children=[];for(let a=0;a<this.children.length;a++)r.children.push(this.children[a].toJSON(e).object)}if(this.animations.length>0){r.animations=[];for(let a=0;a<this.animations.length;a++){let l=this.animations[a];r.animations.push(s(e.animations,l))}}if(t){let a=o(e.geometries),l=o(e.materials),c=o(e.textures),h=o(e.images),d=o(e.shapes),u=o(e.skeletons),p=o(e.animations),x=o(e.nodes);a.length>0&&(i.geometries=a),l.length>0&&(i.materials=l),c.length>0&&(i.textures=c),h.length>0&&(i.images=h),d.length>0&&(i.shapes=d),u.length>0&&(i.skeletons=u),p.length>0&&(i.animations=p),x.length>0&&(i.nodes=x)}return i.object=r,i;function o(a){let l=[];for(let c in a){let h=a[c];delete h.metadata,l.push(h)}return l}}clone(e){return new this.constructor().copy(this,e)}copy(e,t=!0){if(this.name=e.name,this.up.copy(e.up),this.position.copy(e.position),this.rotation.order=e.rotation.order,this.quaternion.copy(e.quaternion),this.scale.copy(e.scale),this.pivot=e.pivot!==null?e.pivot.clone():null,this.matrix.copy(e.matrix),this.matrixWorld.copy(e.matrixWorld),this.matrixAutoUpdate=e.matrixAutoUpdate,this.matrixWorldAutoUpdate=e.matrixWorldAutoUpdate,this.matrixWorldNeedsUpdate=e.matrixWorldNeedsUpdate,this.layers.mask=e.layers.mask,this.visible=e.visible,this.castShadow=e.castShadow,this.receiveShadow=e.receiveShadow,this.frustumCulled=e.frustumCulled,this.renderOrder=e.renderOrder,this.static=e.static,this.animations=e.animations.slice(),this.userData=JSON.parse(JSON.stringify(e.userData)),t===!0)for(let i=0;i<e.children.length;i++){let r=e.children[i];this.add(r.clone())}return this}dispose(){this.dispatchEvent({type:"dispose"})}};yn.DEFAULT_UP=new G(0,1,0);yn.DEFAULT_MATRIX_AUTO_UPDATE=!0;yn.DEFAULT_MATRIX_WORLD_AUTO_UPDATE=!0;var Oe=class extends yn{constructor(){super(),this.isGroup=!0,this.type="Group"}},Zp={type:"move"},ss=class{constructor(){this._targetRay=null,this._grip=null,this._hand=null}getHandSpace(){return this._hand===null&&(this._hand=new Oe,this._hand.matrixAutoUpdate=!1,this._hand.visible=!1,this._hand.joints={},this._hand.inputState={pinching:!1}),this._hand}getTargetRaySpace(){return this._targetRay===null&&(this._targetRay=new Oe,this._targetRay.matrixAutoUpdate=!1,this._targetRay.visible=!1,this._targetRay.hasLinearVelocity=!1,this._targetRay.linearVelocity=new G,this._targetRay.hasAngularVelocity=!1,this._targetRay.angularVelocity=new G),this._targetRay}getGripSpace(){return this._grip===null&&(this._grip=new Oe,this._grip.matrixAutoUpdate=!1,this._grip.visible=!1,this._grip.hasLinearVelocity=!1,this._grip.linearVelocity=new G,this._grip.hasAngularVelocity=!1,this._grip.angularVelocity=new G,this._grip.eventsEnabled=!1),this._grip}dispatchEvent(e){return this._targetRay!==null&&this._targetRay.dispatchEvent(e),this._grip!==null&&this._grip.dispatchEvent(e),this._hand!==null&&this._hand.dispatchEvent(e),this}connect(e){if(e&&e.hand){let t=this._hand;if(t)for(let i of e.hand.values())this._getHandJoint(t,i)}return this.dispatchEvent({type:"connected",data:e}),this}disconnect(e){return this.dispatchEvent({type:"disconnected",data:e}),this._targetRay!==null&&(this._targetRay.visible=!1),this._grip!==null&&(this._grip.visible=!1),this._hand!==null&&(this._hand.visible=!1),this}update(e,t,i){let r=null,s=null,o=null,a=this._targetRay,l=this._grip,c=this._hand;if(e&&t.session.visibilityState!=="visible-blurred"){if(c&&e.hand){o=!0;for(let v of e.hand.values()){let m=t.getJointPose(v,i),f=this._getHandJoint(c,v);m!==null&&(f.matrix.fromArray(m.transform.matrix),f.matrix.decompose(f.position,f.rotation,f.scale),f.matrixWorldNeedsUpdate=!0,f.jointRadius=m.radius),f.visible=m!==null}let h=c.joints["index-finger-tip"],d=c.joints["thumb-tip"],u=h.position.distanceTo(d.position),p=.02,x=.005;c.inputState.pinching&&u>p+x?(c.inputState.pinching=!1,this.dispatchEvent({type:"pinchend",handedness:e.handedness,target:this})):!c.inputState.pinching&&u<=p-x&&(c.inputState.pinching=!0,this.dispatchEvent({type:"pinchstart",handedness:e.handedness,target:this}))}else l!==null&&e.gripSpace&&(s=t.getPose(e.gripSpace,i),s!==null&&(l.matrix.fromArray(s.transform.matrix),l.matrix.decompose(l.position,l.rotation,l.scale),l.matrixWorldNeedsUpdate=!0,s.linearVelocity?(l.hasLinearVelocity=!0,l.linearVelocity.copy(s.linearVelocity)):l.hasLinearVelocity=!1,s.angularVelocity?(l.hasAngularVelocity=!0,l.angularVelocity.copy(s.angularVelocity)):l.hasAngularVelocity=!1,l.eventsEnabled&&l.dispatchEvent({type:"gripUpdated",data:e,target:this})));a!==null&&(r=t.getPose(e.targetRaySpace,i),r===null&&s!==null&&(r=s),r!==null&&(a.matrix.fromArray(r.transform.matrix),a.matrix.decompose(a.position,a.rotation,a.scale),a.matrixWorldNeedsUpdate=!0,r.linearVelocity?(a.hasLinearVelocity=!0,a.linearVelocity.copy(r.linearVelocity)):a.hasLinearVelocity=!1,r.angularVelocity?(a.hasAngularVelocity=!0,a.angularVelocity.copy(r.angularVelocity)):a.hasAngularVelocity=!1,this.dispatchEvent(Zp)))}return a!==null&&(a.visible=r!==null),l!==null&&(l.visible=s!==null),c!==null&&(c.visible=o!==null),this}_getHandJoint(e,t){if(e.joints[t.jointName]===void 0){let i=new Oe;i.matrixAutoUpdate=!1,i.visible=!1,e.joints[t.jointName]=i,e.add(i)}return e.joints[t.jointName]}},dd={aliceblue:15792383,antiquewhite:16444375,aqua:65535,aquamarine:8388564,azure:15794175,beige:16119260,bisque:16770244,black:0,blanchedalmond:16772045,blue:255,blueviolet:9055202,brown:10824234,burlywood:14596231,cadetblue:6266528,chartreuse:8388352,chocolate:13789470,coral:16744272,cornflowerblue:6591981,cornsilk:16775388,crimson:14423100,cyan:65535,darkblue:139,darkcyan:35723,darkgoldenrod:12092939,darkgray:11119017,darkgreen:25600,darkgrey:11119017,darkkhaki:12433259,darkmagenta:9109643,darkolivegreen:5597999,darkorange:16747520,darkorchid:10040012,darkred:9109504,darksalmon:15308410,darkseagreen:9419919,darkslateblue:4734347,darkslategray:3100495,darkslategrey:3100495,darkturquoise:52945,darkviolet:9699539,deeppink:16716947,deepskyblue:49151,dimgray:6908265,dimgrey:6908265,dodgerblue:2003199,firebrick:11674146,floralwhite:16775920,forestgreen:2263842,fuchsia:16711935,gainsboro:14474460,ghostwhite:16316671,gold:16766720,goldenrod:14329120,gray:8421504,green:32768,greenyellow:11403055,grey:8421504,honeydew:15794160,hotpink:16738740,indianred:13458524,indigo:4915330,ivory:16777200,khaki:15787660,lavender:15132410,lavenderblush:16773365,lawngreen:8190976,lemonchiffon:16775885,lightblue:11393254,lightcoral:15761536,lightcyan:14745599,lightgoldenrodyellow:16448210,lightgray:13882323,lightgreen:9498256,lightgrey:13882323,lightpink:16758465,lightsalmon:16752762,lightseagreen:2142890,lightskyblue:8900346,lightslategray:7833753,lightslategrey:7833753,lightsteelblue:11584734,lightyellow:16777184,lime:65280,limegreen:3329330,linen:16445670,magenta:16711935,maroon:8388608,mediumaquamarine:6737322,mediumblue:205,mediumorchid:12211667,mediumpurple:9662683,mediumseagreen:3978097,mediumslateblue:8087790,mediumspringgreen:64154,mediumturquoise:4772300,mediumvioletred:13047173,midnightblue:1644912,mintcream:16121850,mistyrose:16770273,moccasin:16770229,navajowhite:16768685,navy:128,oldlace:16643558,olive:8421376,olivedrab:7048739,orange:16753920,orangered:16729344,orchid:14315734,palegoldenrod:15657130,palegreen:10025880,paleturquoise:11529966,palevioletred:14381203,papayawhip:16773077,peachpuff:16767673,peru:13468991,pink:16761035,plum:14524637,powderblue:11591910,purple:8388736,rebeccapurple:6697881,red:16711680,rosybrown:12357519,royalblue:4286945,saddlebrown:9127187,salmon:16416882,sandybrown:16032864,seagreen:3050327,seashell:16774638,sienna:10506797,silver:12632256,skyblue:8900331,slateblue:6970061,slategray:7372944,slategrey:7372944,snow:16775930,springgreen:65407,steelblue:4620980,tan:13808780,teal:32896,thistle:14204888,tomato:16737095,turquoise:4251856,violet:15631086,wheat:16113331,white:16777215,whitesmoke:16119285,yellow:16776960,yellowgreen:10145074},Gi={h:0,s:0,l:0},No={h:0,s:0,l:0};function ic(n,e,t){return t<0&&(t+=1),t>1&&(t-=1),t<1/6?n+(e-n)*6*t:t<1/2?e:t<2/3?n+(e-n)*6*(2/3-t):n}var ke=class{constructor(e,t,i){return this.isColor=!0,this.r=1,this.g=1,this.b=1,this.set(e,t,i)}set(e,t,i){if(t===void 0&&i===void 0){let r=e;r&&r.isColor?this.copy(r):typeof r=="number"?this.setHex(r):typeof r=="string"&&this.setStyle(r)}else this.setRGB(e,t,i);return this}setScalar(e){return this.r=e,this.g=e,this.b=e,this}setHex(e,t=Pt){return e=Math.floor(e),this.r=(e>>16&255)/255,this.g=(e>>8&255)/255,this.b=(e&255)/255,ht.colorSpaceToWorking(this,t),this}setRGB(e,t,i,r=ht.workingColorSpace){return this.r=e,this.g=t,this.b=i,ht.colorSpaceToWorking(this,r),this}setHSL(e,t,i,r=ht.workingColorSpace){if(e=Jc(e,1),t=lt(t,0,1),i=lt(i,0,1),t===0)this.r=this.g=this.b=i;else{let s=i<=.5?i*(1+t):i+t-i*t,o=2*i-s;this.r=ic(o,s,e+1/3),this.g=ic(o,s,e),this.b=ic(o,s,e-1/3)}return ht.colorSpaceToWorking(this,r),this}setStyle(e,t=Pt){function i(s){s!==void 0&&parseFloat(s)<1&&Ye("Color: Alpha component of "+e+" will be ignored.")}let r;if(r=/^(\w+)\(([^\)]*)\)/.exec(e)){let s,o=r[1],a=r[2];switch(o){case"rgb":case"rgba":if(s=/^\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*(\d*\.?\d+)\s*)?$/.exec(a))return i(s[4]),this.setRGB(Math.min(255,parseInt(s[1],10))/255,Math.min(255,parseInt(s[2],10))/255,Math.min(255,parseInt(s[3],10))/255,t);if(s=/^\s*(\d+)\%\s*,\s*(\d+)\%\s*,\s*(\d+)\%\s*(?:,\s*(\d*\.?\d+)\s*)?$/.exec(a))return i(s[4]),this.setRGB(Math.min(100,parseInt(s[1],10))/100,Math.min(100,parseInt(s[2],10))/100,Math.min(100,parseInt(s[3],10))/100,t);break;case"hsl":case"hsla":if(s=/^\s*(\d*\.?\d+)\s*,\s*(\d*\.?\d+)\%\s*,\s*(\d*\.?\d+)\%\s*(?:,\s*(\d*\.?\d+)\s*)?$/.exec(a))return i(s[4]),this.setHSL(parseFloat(s[1])/360,parseFloat(s[2])/100,parseFloat(s[3])/100,t);break;default:Ye("Color: Unknown color model "+e)}}else if(r=/^\#([A-Fa-f\d]+)$/.exec(e)){let s=r[1],o=s.length;if(o===3)return this.setRGB(parseInt(s.charAt(0),16)/15,parseInt(s.charAt(1),16)/15,parseInt(s.charAt(2),16)/15,t);if(o===6)return this.setHex(parseInt(s,16),t);Ye("Color: Invalid hex color "+e)}else if(e&&e.length>0)return this.setColorName(e,t);return this}setColorName(e,t=Pt){let i=dd[e.toLowerCase()];return i!==void 0?this.setHex(i,t):Ye("Color: Unknown color "+e),this}clone(){return new this.constructor(this.r,this.g,this.b)}copy(e){return this.r=e.r,this.g=e.g,this.b=e.b,this}copySRGBToLinear(e){return this.r=Ci(e.r),this.g=Ci(e.g),this.b=Ci(e.b),this}copyLinearToSRGB(e){return this.r=jr(e.r),this.g=jr(e.g),this.b=jr(e.b),this}convertSRGBToLinear(){return this.copySRGBToLinear(this),this}convertLinearToSRGB(){return this.copyLinearToSRGB(this),this}getHex(e=Pt){return ht.workingToColorSpace(wn.copy(this),e),Math.round(lt(wn.r*255,0,255))*65536+Math.round(lt(wn.g*255,0,255))*256+Math.round(lt(wn.b*255,0,255))}getHexString(e=Pt){return("000000"+this.getHex(e).toString(16)).slice(-6)}getHSL(e,t=ht.workingColorSpace){ht.workingToColorSpace(wn.copy(this),t);let i=wn.r,r=wn.g,s=wn.b,o=Math.max(i,r,s),a=Math.min(i,r,s),l,c,h=(a+o)/2;if(a===o)l=0,c=0;else{let d=o-a;switch(c=h<=.5?d/(o+a):d/(2-o-a),o){case i:l=(r-s)/d+(r<s?6:0);break;case r:l=(s-i)/d+2;break;case s:l=(i-r)/d+4;break}l/=6}return e.h=l,e.s=c,e.l=h,e}getRGB(e,t=ht.workingColorSpace){return ht.workingToColorSpace(wn.copy(this),t),e.r=wn.r,e.g=wn.g,e.b=wn.b,e}getStyle(e=Pt){ht.workingToColorSpace(wn.copy(this),e);let t=wn.r,i=wn.g,r=wn.b;return e!==Pt?`color(${e} ${t.toFixed(3)} ${i.toFixed(3)} ${r.toFixed(3)})`:`rgb(${Math.round(t*255)},${Math.round(i*255)},${Math.round(r*255)})`}offsetHSL(e,t,i){return this.getHSL(Gi),this.setHSL(Gi.h+e,Gi.s+t,Gi.l+i)}add(e){return this.r+=e.r,this.g+=e.g,this.b+=e.b,this}addColors(e,t){return this.r=e.r+t.r,this.g=e.g+t.g,this.b=e.b+t.b,this}addScalar(e){return this.r+=e,this.g+=e,this.b+=e,this}sub(e){return this.r=Math.max(0,this.r-e.r),this.g=Math.max(0,this.g-e.g),this.b=Math.max(0,this.b-e.b),this}multiply(e){return this.r*=e.r,this.g*=e.g,this.b*=e.b,this}multiplyScalar(e){return this.r*=e,this.g*=e,this.b*=e,this}lerp(e,t){return this.r+=(e.r-this.r)*t,this.g+=(e.g-this.g)*t,this.b+=(e.b-this.b)*t,this}lerpColors(e,t,i){return this.r=e.r+(t.r-e.r)*i,this.g=e.g+(t.g-e.g)*i,this.b=e.b+(t.b-e.b)*i,this}lerpHSL(e,t){this.getHSL(Gi),e.getHSL(No);let i=Us(Gi.h,No.h,t),r=Us(Gi.s,No.s,t),s=Us(Gi.l,No.l,t);return this.setHSL(i,r,s),this}setFromVector3(e){return this.r=e.x,this.g=e.y,this.b=e.z,this}applyMatrix3(e){let t=this.r,i=this.g,r=this.b,s=e.elements;return this.r=s[0]*t+s[3]*i+s[6]*r,this.g=s[1]*t+s[4]*i+s[7]*r,this.b=s[2]*t+s[5]*i+s[8]*r,this}equals(e){return e.r===this.r&&e.g===this.g&&e.b===this.b}fromArray(e,t=0){return this.r=e[t],this.g=e[t+1],this.b=e[t+2],this}toArray(e=[],t=0){return e[t]=this.r,e[t+1]=this.g,e[t+2]=this.b,e}fromBufferAttribute(e,t){return this.r=e.getX(t),this.g=e.getY(t),this.b=e.getZ(t),this}toJSON(){return this.getHex()}*[Symbol.iterator](){yield this.r,yield this.g,yield this.b}},wn=new ke;ke.NAMES=dd;var Gs=class n{constructor(e,t=1,i=1e3){this.isFog=!0,this.name="",this.color=new ke(e),this.near=t,this.far=i}clone(){return new n(this.color,this.near,this.far)}toJSON(){return{type:"Fog",name:this.name,color:this.color.getHex(),near:this.near,far:this.far}}},xr=class extends yn{constructor(){super(),this.isScene=!0,this.type="Scene",this.background=null,this.environment=null,this.fog=null,this.backgroundBlurriness=0,this.backgroundIntensity=1,this.backgroundRotation=new $i,this.environmentIntensity=1,this.environmentRotation=new $i,this.overrideMaterial=null,typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("observe",{detail:this}))}copy(e,t){return super.copy(e,t),e.background!==null&&(this.background=e.background.clone()),e.environment!==null&&(this.environment=e.environment.clone()),e.fog!==null&&(this.fog=e.fog.clone()),this.backgroundBlurriness=e.backgroundBlurriness,this.backgroundIntensity=e.backgroundIntensity,this.backgroundRotation.copy(e.backgroundRotation),this.environmentIntensity=e.environmentIntensity,this.environmentRotation.copy(e.environmentRotation),e.overrideMaterial!==null&&(this.overrideMaterial=e.overrideMaterial.clone()),this.matrixAutoUpdate=e.matrixAutoUpdate,this}toJSON(e){let t=super.toJSON(e);return this.fog!==null&&(t.object.fog=this.fog.toJSON()),t.object.backgroundBlurriness=this.backgroundBlurriness,t.object.backgroundIntensity=this.backgroundIntensity,t.object.backgroundRotation=this.backgroundRotation.toArray(),t.object.environmentIntensity=this.environmentIntensity,t.object.environmentRotation=this.environmentRotation.toArray(),t}},si=new G,Ei=new G,rc=new G,Ti=new G,zr=new G,Gr=new G,dh=new G,sc=new G,oc=new G,ac=new G,lc=new Vt,cc=new Vt,uc=new Vt,qi=class n{constructor(e=new G,t=new G,i=new G){this.a=e,this.b=t,this.c=i}static getNormal(e,t,i,r){r.subVectors(i,t),si.subVectors(e,t),r.cross(si);let s=r.lengthSq();return s>0?r.multiplyScalar(1/Math.sqrt(s)):r.set(0,0,0)}static getBarycoord(e,t,i,r,s){si.subVectors(r,t),Ei.subVectors(i,t),rc.subVectors(e,t);let o=si.dot(si),a=si.dot(Ei),l=si.dot(rc),c=Ei.dot(Ei),h=Ei.dot(rc),d=o*c-a*a;if(d===0)return s.set(0,0,0),null;let u=1/d,p=(c*l-a*h)*u,x=(o*h-a*l)*u;return s.set(1-p-x,x,p)}static containsPoint(e,t,i,r){return this.getBarycoord(e,t,i,r,Ti)===null?!1:Ti.x>=0&&Ti.y>=0&&Ti.x+Ti.y<=1}static getInterpolation(e,t,i,r,s,o,a,l){return this.getBarycoord(e,t,i,r,Ti)===null?(l.x=0,l.y=0,"z"in l&&(l.z=0),"w"in l&&(l.w=0),null):(l.setScalar(0),l.addScaledVector(s,Ti.x),l.addScaledVector(o,Ti.y),l.addScaledVector(a,Ti.z),l)}static getInterpolatedAttribute(e,t,i,r,s,o){return lc.setScalar(0),cc.setScalar(0),uc.setScalar(0),lc.fromBufferAttribute(e,t),cc.fromBufferAttribute(e,i),uc.fromBufferAttribute(e,r),o.setScalar(0),o.addScaledVector(lc,s.x),o.addScaledVector(cc,s.y),o.addScaledVector(uc,s.z),o}static isFrontFacing(e,t,i,r){return si.subVectors(i,t),Ei.subVectors(e,t),si.cross(Ei).dot(r)<0}set(e,t,i){return this.a.copy(e),this.b.copy(t),this.c.copy(i),this}setFromPointsAndIndices(e,t,i,r){return this.a.copy(e[t]),this.b.copy(e[i]),this.c.copy(e[r]),this}setFromAttributeAndIndices(e,t,i,r){return this.a.fromBufferAttribute(e,t),this.b.fromBufferAttribute(e,i),this.c.fromBufferAttribute(e,r),this}clone(){return new this.constructor().copy(this)}copy(e){return this.a.copy(e.a),this.b.copy(e.b),this.c.copy(e.c),this}getArea(){return si.subVectors(this.c,this.b),Ei.subVectors(this.a,this.b),si.cross(Ei).length()*.5}getMidpoint(e){return e.addVectors(this.a,this.b).add(this.c).multiplyScalar(1/3)}getNormal(e){return n.getNormal(this.a,this.b,this.c,e)}getPlane(e){return e.setFromCoplanarPoints(this.a,this.b,this.c)}getBarycoord(e,t){return n.getBarycoord(e,this.a,this.b,this.c,t)}getInterpolation(e,t,i,r,s){return n.getInterpolation(e,this.a,this.b,this.c,t,i,r,s)}containsPoint(e){return n.containsPoint(e,this.a,this.b,this.c)}isFrontFacing(e){return n.isFrontFacing(this.a,this.b,this.c,e)}intersectsBox(e){return e.intersectsTriangle(this)}closestPointToPoint(e,t){let i=this.a,r=this.b,s=this.c,o,a;zr.subVectors(r,i),Gr.subVectors(s,i),sc.subVectors(e,i);let l=zr.dot(sc),c=Gr.dot(sc);if(l<=0&&c<=0)return t.copy(i);oc.subVectors(e,r);let h=zr.dot(oc),d=Gr.dot(oc);if(h>=0&&d<=h)return t.copy(r);let u=l*d-h*c;if(u<=0&&l>=0&&h<=0)return o=l/(l-h),t.copy(i).addScaledVector(zr,o);ac.subVectors(e,s);let p=zr.dot(ac),x=Gr.dot(ac);if(x>=0&&p<=x)return t.copy(s);let v=p*c-l*x;if(v<=0&&c>=0&&x<=0)return a=c/(c-x),t.copy(i).addScaledVector(Gr,a);let m=h*x-p*d;if(m<=0&&d-h>=0&&p-x>=0)return dh.subVectors(s,r),a=(d-h)/(d-h+(p-x)),t.copy(r).addScaledVector(dh,a);let f=1/(m+v+u);return o=v*f,a=u*f,t.copy(i).addScaledVector(zr,o).addScaledVector(Gr,a)}equals(e){return e.a.equals(this.a)&&e.b.equals(this.b)&&e.c.equals(this.c)}},Nn=class{constructor(e=new G(1/0,1/0,1/0),t=new G(-1/0,-1/0,-1/0)){this.isBox3=!0,this.min=e,this.max=t}set(e,t){return this.min.copy(e),this.max.copy(t),this}setFromArray(e){this.makeEmpty();for(let t=0,i=e.length;t<i;t+=3)this.expandByPoint(oi.fromArray(e,t));return this}setFromBufferAttribute(e){this.makeEmpty();for(let t=0,i=e.count;t<i;t++)this.expandByPoint(oi.fromBufferAttribute(e,t));return this}setFromPoints(e){this.makeEmpty();for(let t=0,i=e.length;t<i;t++)this.expandByPoint(e[t]);return this}setFromCenterAndSize(e,t){let i=oi.copy(t).multiplyScalar(.5);return this.min.copy(e).sub(i),this.max.copy(e).add(i),this}setFromObject(e,t=!1){return this.makeEmpty(),this.expandByObject(e,t)}clone(){return new this.constructor().copy(this)}copy(e){return this.min.copy(e.min),this.max.copy(e.max),this}makeEmpty(){return this.min.x=this.min.y=this.min.z=1/0,this.max.x=this.max.y=this.max.z=-1/0,this}isEmpty(){return this.max.x<this.min.x||this.max.y<this.min.y||this.max.z<this.min.z}getCenter(e){return this.isEmpty()?e.set(0,0,0):e.addVectors(this.min,this.max).multiplyScalar(.5)}getSize(e){return this.isEmpty()?e.set(0,0,0):e.subVectors(this.max,this.min)}expandByPoint(e){return this.min.min(e),this.max.max(e),this}expandByVector(e){return this.min.sub(e),this.max.add(e),this}expandByScalar(e){return this.min.addScalar(-e),this.max.addScalar(e),this}expandByObject(e,t=!1){e.updateWorldMatrix(!1,!1);let i=e.geometry;if(i!==void 0){let s=i.getAttribute("position");if(t===!0&&s!==void 0&&e.isInstancedMesh!==!0)for(let o=0,a=s.count;o<a;o++)e.isMesh===!0?e.getVertexPosition(o,oi):oi.fromBufferAttribute(s,o),oi.applyMatrix4(e.matrixWorld),this.expandByPoint(oi);else e.boundingBox!==void 0?(e.boundingBox===null&&e.computeBoundingBox(),Uo.copy(e.boundingBox)):(i.boundingBox===null&&i.computeBoundingBox(),Uo.copy(i.boundingBox)),Uo.applyMatrix4(e.matrixWorld),this.union(Uo)}let r=e.children;for(let s=0,o=r.length;s<o;s++)this.expandByObject(r[s],t);return this}containsPoint(e){return e.x>=this.min.x&&e.x<=this.max.x&&e.y>=this.min.y&&e.y<=this.max.y&&e.z>=this.min.z&&e.z<=this.max.z}containsBox(e){return this.min.x<=e.min.x&&e.max.x<=this.max.x&&this.min.y<=e.min.y&&e.max.y<=this.max.y&&this.min.z<=e.min.z&&e.max.z<=this.max.z}getParameter(e,t){return t.set((e.x-this.min.x)/(this.max.x-this.min.x),(e.y-this.min.y)/(this.max.y-this.min.y),(e.z-this.min.z)/(this.max.z-this.min.z))}intersectsBox(e){return e.max.x>=this.min.x&&e.min.x<=this.max.x&&e.max.y>=this.min.y&&e.min.y<=this.max.y&&e.max.z>=this.min.z&&e.min.z<=this.max.z}intersectsSphere(e){return this.clampPoint(e.center,oi),oi.distanceToSquared(e.center)<=e.radius*e.radius}intersectsPlane(e){let t,i;return e.normal.x>0?(t=e.normal.x*this.min.x,i=e.normal.x*this.max.x):(t=e.normal.x*this.max.x,i=e.normal.x*this.min.x),e.normal.y>0?(t+=e.normal.y*this.min.y,i+=e.normal.y*this.max.y):(t+=e.normal.y*this.max.y,i+=e.normal.y*this.min.y),e.normal.z>0?(t+=e.normal.z*this.min.z,i+=e.normal.z*this.max.z):(t+=e.normal.z*this.max.z,i+=e.normal.z*this.min.z),t<=-e.constant&&i>=-e.constant}intersectsTriangle(e){if(this.isEmpty())return!1;this.getCenter(Ps),Fo.subVectors(this.max,Ps),Vr.subVectors(e.a,Ps),Wr.subVectors(e.b,Ps),Xr.subVectors(e.c,Ps),Vi.subVectors(Wr,Vr),Wi.subVectors(Xr,Wr),hr.subVectors(Vr,Xr);let t=[0,-Vi.z,Vi.y,0,-Wi.z,Wi.y,0,-hr.z,hr.y,Vi.z,0,-Vi.x,Wi.z,0,-Wi.x,hr.z,0,-hr.x,-Vi.y,Vi.x,0,-Wi.y,Wi.x,0,-hr.y,hr.x,0];return!hc(t,Vr,Wr,Xr,Fo)||(t=[1,0,0,0,1,0,0,0,1],!hc(t,Vr,Wr,Xr,Fo))?!1:(Oo.crossVectors(Vi,Wi),t=[Oo.x,Oo.y,Oo.z],hc(t,Vr,Wr,Xr,Fo))}clampPoint(e,t){return t.copy(e).clamp(this.min,this.max)}distanceToPoint(e){return this.clampPoint(e,oi).distanceTo(e)}getBoundingSphere(e){return this.isEmpty()?e.makeEmpty():(this.getCenter(e.center),e.radius=this.getSize(oi).length()*.5),e}intersect(e){return this.min.max(e.min),this.max.min(e.max),this.isEmpty()&&this.makeEmpty(),this}union(e){return this.min.min(e.min),this.max.max(e.max),this}applyMatrix4(e){return this.isEmpty()?this:(Ai[0].set(this.min.x,this.min.y,this.min.z).applyMatrix4(e),Ai[1].set(this.min.x,this.min.y,this.max.z).applyMatrix4(e),Ai[2].set(this.min.x,this.max.y,this.min.z).applyMatrix4(e),Ai[3].set(this.min.x,this.max.y,this.max.z).applyMatrix4(e),Ai[4].set(this.max.x,this.min.y,this.min.z).applyMatrix4(e),Ai[5].set(this.max.x,this.min.y,this.max.z).applyMatrix4(e),Ai[6].set(this.max.x,this.max.y,this.min.z).applyMatrix4(e),Ai[7].set(this.max.x,this.max.y,this.max.z).applyMatrix4(e),this.setFromPoints(Ai),this)}translate(e){return this.min.add(e),this.max.add(e),this}equals(e){return e.min.equals(this.min)&&e.max.equals(this.max)}toJSON(){return{min:this.min.toArray(),max:this.max.toArray()}}fromJSON(e){return this.min.fromArray(e.min),this.max.fromArray(e.max),this}},Ai=[new G,new G,new G,new G,new G,new G,new G,new G],oi=new G,Uo=new Nn,Vr=new G,Wr=new G,Xr=new G,Vi=new G,Wi=new G,hr=new G,Ps=new G,Fo=new G,Oo=new G,dr=new G;function hc(n,e,t,i,r){for(let s=0,o=n.length-3;s<=o;s+=3){dr.fromArray(n,s);let a=r.x*Math.abs(dr.x)+r.y*Math.abs(dr.y)+r.z*Math.abs(dr.z),l=e.dot(dr),c=t.dot(dr),h=i.dot(dr);if(Math.max(-Math.max(l,c,h),Math.min(l,c,h))>a)return!1}return!0}var on=new G,Bo=new Xe,Kp=0,Cn=class extends li{constructor(e,t,i=!1){if(super(),Array.isArray(e))throw new TypeError("THREE.BufferAttribute: array should be a Typed Array.");this.isBufferAttribute=!0,Object.defineProperty(this,"id",{value:Kp++}),this.name="",this.array=e,this.itemSize=t,this.count=e!==void 0?e.length/t:0,this.normalized=i,this.usage=ad,this.updateRanges=[],this.gpuType=Jn,this.version=0}onUploadCallback(){}set needsUpdate(e){e===!0&&this.version++}setUsage(e){return this.usage=e,this}addUpdateRange(e,t){this.updateRanges.push({start:e,count:t})}clearUpdateRanges(){this.updateRanges.length=0}copy(e){return this.name=e.name,this.array=new e.array.constructor(e.array),this.itemSize=e.itemSize,this.count=e.count,this.normalized=e.normalized,this.usage=e.usage,this.gpuType=e.gpuType,this}copyAt(e,t,i){e*=this.itemSize,i*=t.itemSize;for(let r=0,s=this.itemSize;r<s;r++)this.array[e+r]=t.array[i+r];return this}copyArray(e){return this.array.set(e),this}applyMatrix3(e){if(this.itemSize===2)for(let t=0,i=this.count;t<i;t++)Bo.fromBufferAttribute(this,t),Bo.applyMatrix3(e),this.setXY(t,Bo.x,Bo.y);else if(this.itemSize===3)for(let t=0,i=this.count;t<i;t++)on.fromBufferAttribute(this,t),on.applyMatrix3(e),this.setXYZ(t,on.x,on.y,on.z);return this}applyMatrix4(e){for(let t=0,i=this.count;t<i;t++)on.fromBufferAttribute(this,t),on.applyMatrix4(e),this.setXYZ(t,on.x,on.y,on.z);return this}applyNormalMatrix(e){for(let t=0,i=this.count;t<i;t++)on.fromBufferAttribute(this,t),on.applyNormalMatrix(e),this.setXYZ(t,on.x,on.y,on.z);return this}transformDirection(e){for(let t=0,i=this.count;t<i;t++)on.fromBufferAttribute(this,t),on.transformDirection(e),this.setXYZ(t,on.x,on.y,on.z);return this}set(e,t=0){return this.array.set(e,t),this}getComponent(e,t){let i=this.array[e*this.itemSize+t];return this.normalized&&(i=Jr(i,this.array)),i}setComponent(e,t,i){return this.normalized&&(i=Rn(i,this.array)),this.array[e*this.itemSize+t]=i,this}getX(e){let t=this.array[e*this.itemSize];return this.normalized&&(t=Jr(t,this.array)),t}setX(e,t){return this.normalized&&(t=Rn(t,this.array)),this.array[e*this.itemSize]=t,this}getY(e){let t=this.array[e*this.itemSize+1];return this.normalized&&(t=Jr(t,this.array)),t}setY(e,t){return this.normalized&&(t=Rn(t,this.array)),this.array[e*this.itemSize+1]=t,this}getZ(e){let t=this.array[e*this.itemSize+2];return this.normalized&&(t=Jr(t,this.array)),t}setZ(e,t){return this.normalized&&(t=Rn(t,this.array)),this.array[e*this.itemSize+2]=t,this}getW(e){let t=this.array[e*this.itemSize+3];return this.normalized&&(t=Jr(t,this.array)),t}setW(e,t){return this.normalized&&(t=Rn(t,this.array)),this.array[e*this.itemSize+3]=t,this}setXY(e,t,i){return e*=this.itemSize,this.normalized&&(t=Rn(t,this.array),i=Rn(i,this.array)),this.array[e+0]=t,this.array[e+1]=i,this}setXYZ(e,t,i,r){return e*=this.itemSize,this.normalized&&(t=Rn(t,this.array),i=Rn(i,this.array),r=Rn(r,this.array)),this.array[e+0]=t,this.array[e+1]=i,this.array[e+2]=r,this}setXYZW(e,t,i,r,s){return e*=this.itemSize,this.normalized&&(t=Rn(t,this.array),i=Rn(i,this.array),r=Rn(r,this.array),s=Rn(s,this.array)),this.array[e+0]=t,this.array[e+1]=i,this.array[e+2]=r,this.array[e+3]=s,this}onUpload(e){return this.onUploadCallback=e,this}clone(){return new this.constructor(this.array,this.itemSize).copy(this)}toJSON(){let e={itemSize:this.itemSize,type:this.array.constructor.name,array:Array.from(this.array),normalized:this.normalized};return e.name=this.name,e.usage=this.usage,e.gpuType=this.gpuType,e}dispose(){this.dispatchEvent({type:"dispose"})}};var Vs=class extends Cn{constructor(e,t,i){super(new Uint16Array(e),t,i)}};var Ws=class extends Cn{constructor(e,t,i){super(new Uint32Array(e),t,i)}};var _t=class extends Cn{constructor(e,t,i){super(new Float32Array(e),t,i)}},Jp=new Nn,Is=new G,dc=new G,Pi=class{constructor(e=new G,t=-1){this.isSphere=!0,this.center=e,this.radius=t}set(e,t){return this.center.copy(e),this.radius=t,this}setFromPoints(e,t){let i=this.center;t!==void 0?i.copy(t):Jp.setFromPoints(e).getCenter(i);let r=0;for(let s=0,o=e.length;s<o;s++)r=Math.max(r,i.distanceToSquared(e[s]));return this.radius=Math.sqrt(r),this}copy(e){return this.center.copy(e.center),this.radius=e.radius,this}isEmpty(){return this.radius<0}makeEmpty(){return this.center.set(0,0,0),this.radius=-1,this}containsPoint(e){return e.distanceToSquared(this.center)<=this.radius*this.radius}distanceToPoint(e){return e.distanceTo(this.center)-this.radius}intersectsSphere(e){let t=this.radius+e.radius;return e.center.distanceToSquared(this.center)<=t*t}intersectsBox(e){return e.intersectsSphere(this)}intersectsPlane(e){return Math.abs(e.distanceToPoint(this.center))<=this.radius}clampPoint(e,t){let i=this.center.distanceToSquared(e);return t.copy(e),i>this.radius*this.radius&&(t.sub(this.center).normalize(),t.multiplyScalar(this.radius).add(this.center)),t}getBoundingBox(e){return this.isEmpty()?(e.makeEmpty(),e):(e.set(this.center,this.center),e.expandByScalar(this.radius),e)}applyMatrix4(e){return this.center.applyMatrix4(e),this.radius=this.radius*e.getMaxScaleOnAxis(),this}translate(e){return this.center.add(e),this}expandByPoint(e){if(this.isEmpty())return this.center.copy(e),this.radius=0,this;Is.subVectors(e,this.center);let t=Is.lengthSq();if(t>this.radius*this.radius){let i=Math.sqrt(t),r=(i-this.radius)*.5;this.center.addScaledVector(Is,r/i),this.radius+=r}return this}union(e){return e.isEmpty()?this:this.isEmpty()?(this.copy(e),this):(this.center.equals(e.center)===!0?this.radius=Math.max(this.radius,e.radius):(dc.subVectors(e.center,this.center).setLength(e.radius),this.expandByPoint(Is.copy(e.center).add(dc)),this.expandByPoint(Is.copy(e.center).sub(dc))),this)}equals(e){return e.center.equals(this.center)&&e.radius===this.radius}clone(){return new this.constructor().copy(this)}toJSON(){return{radius:this.radius,center:this.center.toArray()}}fromJSON(e){return this.radius=e.radius,this.center.fromArray(e.center),this}},jp=0,Kn=new wt,fc=new yn,qr=new G,Vn=new Nn,ks=new Nn,pn=new G,Yt=class n extends li{constructor(){super(),this.isBufferGeometry=!0,Object.defineProperty(this,"id",{value:jp++}),this.uuid=ms(),this.name="",this.type="BufferGeometry",this.index=null,this.indirect=null,this.indirectOffset=0,this.attributes={},this.morphAttributes={},this.morphTargetsRelative=!1,this.groups=[],this.boundingBox=null,this.boundingSphere=null,this.drawRange={start:0,count:1/0},this.userData={},this._transformed=!1}getIndex(){return this.index}setIndex(e){return Array.isArray(e)?this.index=new(Mp(e)?Ws:Vs)(e,1):this.index=e,this}setIndirect(e,t=0){return this.indirect=e,this.indirectOffset=t,this}getIndirect(){return this.indirect}getAttribute(e){return this.attributes[e]}setAttribute(e,t){return this.attributes[e]=t,this}deleteAttribute(e){return delete this.attributes[e],this}hasAttribute(e){return this.attributes[e]!==void 0}addGroup(e,t,i=0){this.groups.push({start:e,count:t,materialIndex:i})}clearGroups(){this.groups=[]}setDrawRange(e,t){this.drawRange.start=e,this.drawRange.count=t}applyMatrix4(e){let t=this.attributes.position;t!==void 0&&(t.applyMatrix4(e),t.needsUpdate=!0);let i=this.attributes.normal;if(i!==void 0){let s=new je().getNormalMatrix(e);i.applyNormalMatrix(s),i.needsUpdate=!0}let r=this.attributes.tangent;return r!==void 0&&(r.transformDirection(e),r.needsUpdate=!0),this.boundingBox!==null&&this.computeBoundingBox(),this.boundingSphere!==null&&this.computeBoundingSphere(),this._transformed=!0,this}applyQuaternion(e){return Kn.makeRotationFromQuaternion(e),this.applyMatrix4(Kn),this}rotateX(e){return Kn.makeRotationX(e),this.applyMatrix4(Kn),this}rotateY(e){return Kn.makeRotationY(e),this.applyMatrix4(Kn),this}rotateZ(e){return Kn.makeRotationZ(e),this.applyMatrix4(Kn),this}translate(e,t,i){return Kn.makeTranslation(e,t,i),this.applyMatrix4(Kn),this}scale(e,t,i){return Kn.makeScale(e,t,i),this.applyMatrix4(Kn),this}lookAt(e){return fc.lookAt(e),fc.updateMatrix(),this.applyMatrix4(fc.matrix),this}center(){return this.computeBoundingBox(),this.boundingBox.getCenter(qr).negate(),this.translate(qr.x,qr.y,qr.z),this}setFromPoints(e){let t=this.getAttribute("position");if(t===void 0){let i=[];for(let r=0,s=e.length;r<s;r++){let o=e[r];i.push(o.x,o.y,o.z||0)}this.setAttribute("position",new _t(i,3))}else{let i=Math.min(e.length,t.count);for(let r=0;r<i;r++){let s=e[r];t.setXYZ(r,s.x,s.y,s.z||0)}e.length>t.count&&Ye("BufferGeometry: Buffer size too small for points data. Use .dispose() and create a new geometry."),t.needsUpdate=!0}return this}computeBoundingBox(){this.boundingBox===null&&(this.boundingBox=new Nn);let e=this.attributes.position,t=this.morphAttributes.position;if(e&&e.isGLBufferAttribute){Ke("BufferGeometry.computeBoundingBox(): GLBufferAttribute requires a manual bounding box.",this),this.boundingBox.set(new G(-1/0,-1/0,-1/0),new G(1/0,1/0,1/0));return}if(e!==void 0){if(this.boundingBox.setFromBufferAttribute(e),t)for(let i=0,r=t.length;i<r;i++){let s=t[i];Vn.setFromBufferAttribute(s),this.morphTargetsRelative?(pn.addVectors(this.boundingBox.min,Vn.min),this.boundingBox.expandByPoint(pn),pn.addVectors(this.boundingBox.max,Vn.max),this.boundingBox.expandByPoint(pn)):(this.boundingBox.expandByPoint(Vn.min),this.boundingBox.expandByPoint(Vn.max))}}else this.boundingBox.makeEmpty();(isNaN(this.boundingBox.min.x)||isNaN(this.boundingBox.min.y)||isNaN(this.boundingBox.min.z))&&Ke('BufferGeometry.computeBoundingBox(): Computed min/max have NaN values. The "position" attribute is likely to have NaN values.',this)}computeBoundingSphere(){this.boundingSphere===null&&(this.boundingSphere=new Pi);let e=this.attributes.position,t=this.morphAttributes.position;if(e&&e.isGLBufferAttribute){Ke("BufferGeometry.computeBoundingSphere(): GLBufferAttribute requires a manual bounding sphere.",this),this.boundingSphere.set(new G,1/0);return}if(e){let i=this.boundingSphere.center;if(Vn.setFromBufferAttribute(e),t)for(let s=0,o=t.length;s<o;s++){let a=t[s];ks.setFromBufferAttribute(a),this.morphTargetsRelative?(pn.addVectors(Vn.min,ks.min),Vn.expandByPoint(pn),pn.addVectors(Vn.max,ks.max),Vn.expandByPoint(pn)):(Vn.expandByPoint(ks.min),Vn.expandByPoint(ks.max))}Vn.getCenter(i);let r=0;for(let s=0,o=e.count;s<o;s++)pn.fromBufferAttribute(e,s),r=Math.max(r,i.distanceToSquared(pn));if(t)for(let s=0,o=t.length;s<o;s++){let a=t[s],l=this.morphTargetsRelative;for(let c=0,h=a.count;c<h;c++)pn.fromBufferAttribute(a,c),l&&(qr.fromBufferAttribute(e,c),pn.add(qr)),r=Math.max(r,i.distanceToSquared(pn))}this.boundingSphere.radius=Math.sqrt(r),isNaN(this.boundingSphere.radius)&&Ke('BufferGeometry.computeBoundingSphere(): Computed radius is NaN. The "position" attribute is likely to have NaN values.',this)}}computeTangents(){let e=this.index,t=this.attributes;if(e===null||t.position===void 0||t.normal===void 0||t.uv===void 0){Ke("BufferGeometry: .computeTangents() failed. Missing required attributes (index, position, normal or uv)");return}let i=t.position,r=t.normal,s=t.uv,o=this.getAttribute("tangent");(o===void 0||o.count!==i.count)&&(o=new Cn(new Float32Array(4*i.count),4),this.setAttribute("tangent",o));let a=[],l=[];for(let g=0;g<i.count;g++)a[g]=new G,l[g]=new G;let c=new G,h=new G,d=new G,u=new Xe,p=new Xe,x=new Xe,v=new G,m=new G;function f(g,S,_){c.fromBufferAttribute(i,g),h.fromBufferAttribute(i,S),d.fromBufferAttribute(i,_),u.fromBufferAttribute(s,g),p.fromBufferAttribute(s,S),x.fromBufferAttribute(s,_),h.sub(c),d.sub(c),p.sub(u),x.sub(u);let A=1/(p.x*x.y-x.x*p.y);isFinite(A)&&(v.copy(h).multiplyScalar(x.y).addScaledVector(d,-p.y).multiplyScalar(A),m.copy(d).multiplyScalar(p.x).addScaledVector(h,-x.x).multiplyScalar(A),a[g].add(v),a[S].add(v),a[_].add(v),l[g].add(m),l[S].add(m),l[_].add(m))}let b=this.groups;b.length===0&&(b=[{start:0,count:e.count}]);for(let g=0,S=b.length;g<S;++g){let _=b[g],A=_.start,I=_.count;for(let k=A,N=A+I;k<N;k+=3)f(e.getX(k+0),e.getX(k+1),e.getX(k+2))}let P=new G,w=new G,T=new G,E=new G;function D(g){T.fromBufferAttribute(r,g),E.copy(T);let S=a[g];P.copy(S),P.sub(T.multiplyScalar(T.dot(S))).normalize(),w.crossVectors(E,S);let A=w.dot(l[g])<0?-1:1;o.setXYZW(g,P.x,P.y,P.z,A)}for(let g=0,S=b.length;g<S;++g){let _=b[g],A=_.start,I=_.count;for(let k=A,N=A+I;k<N;k+=3)D(e.getX(k+0)),D(e.getX(k+1)),D(e.getX(k+2))}this._transformed=!0}computeVertexNormals(){let e=this.index,t=this.getAttribute("position");if(t!==void 0){let i=this.getAttribute("normal");if(i===void 0||i.count!==t.count)i=new Cn(new Float32Array(t.count*3),3),this.setAttribute("normal",i);else for(let u=0,p=i.count;u<p;u++)i.setXYZ(u,0,0,0);let r=new G,s=new G,o=new G,a=new G,l=new G,c=new G,h=new G,d=new G;if(e)for(let u=0,p=e.count;u<p;u+=3){let x=e.getX(u+0),v=e.getX(u+1),m=e.getX(u+2);r.fromBufferAttribute(t,x),s.fromBufferAttribute(t,v),o.fromBufferAttribute(t,m),h.subVectors(o,s),d.subVectors(r,s),h.cross(d),a.fromBufferAttribute(i,x),l.fromBufferAttribute(i,v),c.fromBufferAttribute(i,m),a.add(h),l.add(h),c.add(h),i.setXYZ(x,a.x,a.y,a.z),i.setXYZ(v,l.x,l.y,l.z),i.setXYZ(m,c.x,c.y,c.z)}else for(let u=0,p=t.count;u<p;u+=3)r.fromBufferAttribute(t,u+0),s.fromBufferAttribute(t,u+1),o.fromBufferAttribute(t,u+2),h.subVectors(o,s),d.subVectors(r,s),h.cross(d),i.setXYZ(u+0,h.x,h.y,h.z),i.setXYZ(u+1,h.x,h.y,h.z),i.setXYZ(u+2,h.x,h.y,h.z);this.normalizeNormals(),i.needsUpdate=!0}}normalizeNormals(){let e=this.attributes.normal;for(let t=0,i=e.count;t<i;t++)pn.fromBufferAttribute(e,t),pn.normalize(),e.setXYZ(t,pn.x,pn.y,pn.z)}toNonIndexed(){function e(a,l){let c=a.array,h=a.itemSize,d=a.normalized,u=new c.constructor(l.length*h),p=0,x=0;for(let v=0,m=l.length;v<m;v++){a.isInterleavedBufferAttribute?p=l[v]*a.data.stride+a.offset:p=l[v]*h;for(let f=0;f<h;f++)u[x++]=c[p++]}return new Cn(u,h,d)}if(this.index===null)return Ye("BufferGeometry.toNonIndexed(): BufferGeometry is already non-indexed."),this;let t=new n,i=this.index.array,r=this.attributes;for(let a in r){let l=r[a],c=e(l,i);t.setAttribute(a,c)}let s=this.morphAttributes;for(let a in s){let l=[],c=s[a];for(let h=0,d=c.length;h<d;h++){let u=c[h],p=e(u,i);l.push(p)}t.morphAttributes[a]=l}t.morphTargetsRelative=this.morphTargetsRelative;let o=this.groups;for(let a=0,l=o.length;a<l;a++){let c=o[a];t.addGroup(c.start,c.count,c.materialIndex)}return t}toJSON(){let e={metadata:{version:4.7,type:"BufferGeometry",generator:"BufferGeometry.toJSON"}};if(e.uuid=this.uuid,e.type=this.parameters!==void 0&&this._transformed===!0?"BufferGeometry":this.type,e.name=this.name,Object.keys(this.userData).length>0&&(e.userData=this.userData),this.parameters!==void 0&&this._transformed!==!0){let l=this.parameters;for(let c in l)l[c]!==void 0&&(e[c]=l[c]);return e}e.data={attributes:{}};let t=this.index;t!==null&&(e.data.index={type:t.array.constructor.name,array:Array.prototype.slice.call(t.array)});let i=this.attributes;for(let l in i){let c=i[l];e.data.attributes[l]=c.toJSON(e.data)}let r={},s=!1;for(let l in this.morphAttributes){let c=this.morphAttributes[l],h=[];for(let d=0,u=c.length;d<u;d++){let p=c[d];h.push(p.toJSON(e.data))}h.length>0&&(r[l]=h,s=!0)}s&&(e.data.morphAttributes=r,e.data.morphTargetsRelative=this.morphTargetsRelative);let o=this.groups;o.length>0&&(e.data.groups=JSON.parse(JSON.stringify(o)));let a=this.boundingSphere;return a!==null&&(e.data.boundingSphere=a.toJSON()),e}clone(){return new this.constructor().copy(this)}copy(e){this.index=null,this.attributes={},this.morphAttributes={},this.groups=[],this.boundingBox=null,this.boundingSphere=null;let t={};this.name=e.name;let i=e.index;i!==null&&this.setIndex(i.clone());let r=e.attributes;for(let c in r){let h=r[c];this.setAttribute(c,h.clone(t))}let s=e.morphAttributes;for(let c in s){let h=[],d=s[c];for(let u=0,p=d.length;u<p;u++)h.push(d[u].clone(t));this.morphAttributes[c]=h}this.morphTargetsRelative=e.morphTargetsRelative;let o=e.groups;for(let c=0,h=o.length;c<h;c++){let d=o[c];this.addGroup(d.start,d.count,d.materialIndex)}let a=e.boundingBox;a!==null&&(this.boundingBox=a.clone());let l=e.boundingSphere;return l!==null&&(this.boundingSphere=l.clone()),this.drawRange.start=e.drawRange.start,this.drawRange.count=e.drawRange.count,this.userData=e.userData,this._transformed=e._transformed,this}dispose(){this.dispatchEvent({type:"dispose"})}};var pc=new G,Qp=new G,em=new je,Wn=class{constructor(e=new G(1,0,0),t=0){this.isPlane=!0,this.normal=e,this.constant=t}set(e,t){return this.normal.copy(e),this.constant=t,this}setComponents(e,t,i,r){return this.normal.set(e,t,i),this.constant=r,this}setFromNormalAndCoplanarPoint(e,t){return this.normal.copy(e),this.constant=-t.dot(this.normal),this}setFromCoplanarPoints(e,t,i){let r=pc.subVectors(i,t).cross(Qp.subVectors(e,t)).normalize();return this.setFromNormalAndCoplanarPoint(r,e),this}copy(e){return this.normal.copy(e.normal),this.constant=e.constant,this}normalize(){let e=1/this.normal.length();return this.normal.multiplyScalar(e),this.constant*=e,this}negate(){return this.constant*=-1,this.normal.negate(),this}distanceToPoint(e){return this.normal.dot(e)+this.constant}distanceToSphere(e){return this.distanceToPoint(e.center)-e.radius}projectPoint(e,t){return t.copy(e).addScaledVector(this.normal,-this.distanceToPoint(e))}intersectLine(e,t,i=!0){let r=e.delta(pc),s=this.normal.dot(r);if(s===0)return this.distanceToPoint(e.start)===0?t.copy(e.start):null;let o=-(e.start.dot(this.normal)+this.constant)/s;return i===!0&&(o<0||o>1)?null:t.copy(e.start).addScaledVector(r,o)}intersectsLine(e){let t=this.distanceToPoint(e.start),i=this.distanceToPoint(e.end);return t<0&&i>0||i<0&&t>0}intersectsBox(e){return e.intersectsPlane(this)}intersectsSphere(e){return e.intersectsPlane(this)}coplanarPoint(e){return e.copy(this.normal).multiplyScalar(-this.constant)}applyMatrix4(e,t){let i=t||em.getNormalMatrix(e),r=this.coplanarPoint(pc).applyMatrix4(e),s=this.normal.applyMatrix3(i).normalize();return this.constant=-r.dot(s),this}translate(e){return this.constant-=e.dot(this.normal),this}equals(e){return e.normal.equals(this.normal)&&e.constant===this.constant}clone(){return new this.constructor().copy(this)}toJSON(){return{normal:this.normal.toArray(),constant:this.constant}}fromJSON(e){return this.normal.fromArray(e.normal),this.constant=e.constant,this}},tm=0,yi=class extends li{constructor(){super(),this.isMaterial=!0,Object.defineProperty(this,"id",{value:tm++}),this.uuid=ms(),this.name="",this.type="Material",this.blending=ds,this.side=tr,this.vertexColors=!1,this.opacity=1,this.transparent=!1,this.alphaHash=!1,this.blendSrc=kc,this.blendDst=Lc,this.blendEquation=Sr,this.blendSrcAlpha=null,this.blendDstAlpha=null,this.blendEquationAlpha=null,this.blendColor=new ke(0,0,0),this.blendAlpha=0,this.depthFunc=Qr,this.depthTest=!0,this.depthWrite=!0,this.stencilWriteMask=255,this.stencilFunc=ed,this.stencilRef=0,this.stencilFuncMask=255,this.stencilFail=na,this.stencilZFail=na,this.stencilZPass=na,this.stencilWrite=!1,this.clippingPlanes=null,this.clipIntersection=!1,this.clipShadows=!1,this.shadowSide=null,this.colorWrite=!0,this.precision=null,this.polygonOffset=!1,this.polygonOffsetFactor=0,this.polygonOffsetUnits=0,this.dithering=!1,this.alphaToCoverage=!1,this.premultipliedAlpha=!1,this.forceSinglePass=!1,this.allowOverride=!0,this.visible=!0,this.toneMapped=!0,this.userData={},this.version=0,this._alphaTest=0}get alphaTest(){return this._alphaTest}set alphaTest(e){this._alphaTest>0!=e>0&&this.version++,this._alphaTest=e}onBeforeRender(){}onBeforeCompile(){}customProgramCacheKey(){return this.onBeforeCompile.toString()}setValues(e){if(e!==void 0)for(let t in e){let i=e[t];if(i===void 0){Ye(`Material: parameter '${t}' has value of undefined.`);continue}let r=this[t];if(r===void 0){Ye(`Material: '${t}' is not a property of THREE.${this.type}.`);continue}r&&r.isColor?r.set(i):r&&r.isVector2&&i&&i.isVector2||r&&r.isEuler&&i&&i.isEuler||r&&r.isVector3&&i&&i.isVector3?r.copy(i):this[t]=i}}toJSON(e){let t=e===void 0||typeof e=="string";t&&(e={textures:{},images:{}});let i={metadata:{version:4.7,type:"Material",generator:"Material.toJSON"}};i.uuid=this.uuid,i.type=this.type,i.blending=this.blending,i.side=this.side,i.shadowSide=this.shadowSide,i.vertexColors=this.vertexColors,i.opacity=this.opacity,i.transparent=this.transparent,i.blendSrc=this.blendSrc,i.blendDst=this.blendDst,i.blendEquation=this.blendEquation,i.blendSrcAlpha=this.blendSrcAlpha,i.blendDstAlpha=this.blendDstAlpha,i.blendEquationAlpha=this.blendEquationAlpha,i.blendColor=this.blendColor.getHex(),i.blendAlpha=this.blendAlpha,i.depthFunc=this.depthFunc,i.depthTest=this.depthTest,i.depthWrite=this.depthWrite,i.colorWrite=this.colorWrite,i.clipIntersection=this.clipIntersection,i.clipShadows=this.clipShadows,i.stencilWriteMask=this.stencilWriteMask,i.stencilFunc=this.stencilFunc,i.stencilRef=this.stencilRef,i.stencilFuncMask=this.stencilFuncMask,i.stencilFail=this.stencilFail,i.stencilZFail=this.stencilZFail,i.stencilZPass=this.stencilZPass,i.stencilWrite=this.stencilWrite,i.polygonOffset=this.polygonOffset,i.polygonOffsetFactor=this.polygonOffsetFactor,i.polygonOffsetUnits=this.polygonOffsetUnits,i.dithering=this.dithering,i.alphaTest=this.alphaTest,i.alphaHash=this.alphaHash,i.alphaToCoverage=this.alphaToCoverage,i.premultipliedAlpha=this.premultipliedAlpha,i.forceSinglePass=this.forceSinglePass,i.allowOverride=this.allowOverride,i.visible=this.visible,i.toneMapped=this.toneMapped,i.name=this.name,this.color&&this.color.isColor&&(i.color=this.color.getHex()),this.roughness!==void 0&&(i.roughness=this.roughness),this.metalness!==void 0&&(i.metalness=this.metalness),this.sheen!==void 0&&(i.sheen=this.sheen),this.sheenColor&&this.sheenColor.isColor&&(i.sheenColor=this.sheenColor.getHex()),this.sheenRoughness!==void 0&&(i.sheenRoughness=this.sheenRoughness),this.emissive&&this.emissive.isColor&&(i.emissive=this.emissive.getHex()),this.emissiveIntensity!==void 0&&(i.emissiveIntensity=this.emissiveIntensity),this.specular&&this.specular.isColor&&(i.specular=this.specular.getHex()),this.specularIntensity!==void 0&&(i.specularIntensity=this.specularIntensity),this.specularColor&&this.specularColor.isColor&&(i.specularColor=this.specularColor.getHex()),this.shininess!==void 0&&(i.shininess=this.shininess),this.clearcoat!==void 0&&(i.clearcoat=this.clearcoat),this.clearcoatRoughness!==void 0&&(i.clearcoatRoughness=this.clearcoatRoughness),this.clearcoatMap&&this.clearcoatMap.isTexture&&(i.clearcoatMap=this.clearcoatMap.toJSON(e).uuid),this.clearcoatRoughnessMap&&this.clearcoatRoughnessMap.isTexture&&(i.clearcoatRoughnessMap=this.clearcoatRoughnessMap.toJSON(e).uuid),this.clearcoatNormalMap&&this.clearcoatNormalMap.isTexture&&(i.clearcoatNormalMap=this.clearcoatNormalMap.toJSON(e).uuid,i.clearcoatNormalScale=this.clearcoatNormalScale.toArray()),this.sheenColorMap&&this.sheenColorMap.isTexture&&(i.sheenColorMap=this.sheenColorMap.toJSON(e).uuid),this.sheenRoughnessMap&&this.sheenRoughnessMap.isTexture&&(i.sheenRoughnessMap=this.sheenRoughnessMap.toJSON(e).uuid),this.dispersion!==void 0&&(i.dispersion=this.dispersion),this.retroreflectivity!==void 0&&(i.retroreflectivity=this.retroreflectivity),this.iridescence!==void 0&&(i.iridescence=this.iridescence),this.iridescenceIOR!==void 0&&(i.iridescenceIOR=this.iridescenceIOR),this.iridescenceThicknessRange!==void 0&&(i.iridescenceThicknessRange=this.iridescenceThicknessRange),this.iridescenceMap&&this.iridescenceMap.isTexture&&(i.iridescenceMap=this.iridescenceMap.toJSON(e).uuid),this.iridescenceThicknessMap&&this.iridescenceThicknessMap.isTexture&&(i.iridescenceThicknessMap=this.iridescenceThicknessMap.toJSON(e).uuid),this.anisotropy!==void 0&&(i.anisotropy=this.anisotropy),this.anisotropyRotation!==void 0&&(i.anisotropyRotation=this.anisotropyRotation),this.anisotropyMap&&this.anisotropyMap.isTexture&&(i.anisotropyMap=this.anisotropyMap.toJSON(e).uuid),this.map&&this.map.isTexture&&(i.map=this.map.toJSON(e).uuid),this.matcap&&this.matcap.isTexture&&(i.matcap=this.matcap.toJSON(e).uuid),this.alphaMap&&this.alphaMap.isTexture&&(i.alphaMap=this.alphaMap.toJSON(e).uuid),this.lightMap&&this.lightMap.isTexture&&(i.lightMap=this.lightMap.toJSON(e).uuid,i.lightMapIntensity=this.lightMapIntensity),this.aoMap&&this.aoMap.isTexture&&(i.aoMap=this.aoMap.toJSON(e).uuid,i.aoMapIntensity=this.aoMapIntensity),this.bumpMap&&this.bumpMap.isTexture&&(i.bumpMap=this.bumpMap.toJSON(e).uuid,i.bumpScale=this.bumpScale),this.normalMap&&this.normalMap.isTexture&&(i.normalMap=this.normalMap.toJSON(e).uuid,i.normalMapType=this.normalMapType,i.normalScale=this.normalScale.toArray()),this.displacementMap&&this.displacementMap.isTexture&&(i.displacementMap=this.displacementMap.toJSON(e).uuid,i.displacementScale=this.displacementScale,i.displacementBias=this.displacementBias),this.roughnessMap&&this.roughnessMap.isTexture&&(i.roughnessMap=this.roughnessMap.toJSON(e).uuid),this.metalnessMap&&this.metalnessMap.isTexture&&(i.metalnessMap=this.metalnessMap.toJSON(e).uuid),this.emissiveMap&&this.emissiveMap.isTexture&&(i.emissiveMap=this.emissiveMap.toJSON(e).uuid),this.specularMap&&this.specularMap.isTexture&&(i.specularMap=this.specularMap.toJSON(e).uuid),this.specularIntensityMap&&this.specularIntensityMap.isTexture&&(i.specularIntensityMap=this.specularIntensityMap.toJSON(e).uuid),this.specularColorMap&&this.specularColorMap.isTexture&&(i.specularColorMap=this.specularColorMap.toJSON(e).uuid),this.envMap&&this.envMap.isTexture&&(i.envMap=this.envMap.toJSON(e).uuid,this.combine!==void 0&&(i.combine=this.combine)),this.envMapRotation!==void 0&&(i.envMapRotation=this.envMapRotation.toArray()),this.envMapIntensity!==void 0&&(i.envMapIntensity=this.envMapIntensity),this.reflectivity!==void 0&&(i.reflectivity=this.reflectivity),this.refractionRatio!==void 0&&(i.refractionRatio=this.refractionRatio),this.gradientMap&&this.gradientMap.isTexture&&(i.gradientMap=this.gradientMap.toJSON(e).uuid),this.transmission!==void 0&&(i.transmission=this.transmission),this.transmissionMap&&this.transmissionMap.isTexture&&(i.transmissionMap=this.transmissionMap.toJSON(e).uuid),this.thickness!==void 0&&(i.thickness=this.thickness),this.thicknessMap&&this.thicknessMap.isTexture&&(i.thicknessMap=this.thicknessMap.toJSON(e).uuid),this.attenuationDistance!==void 0&&(i.attenuationDistance=this.attenuationDistance),this.attenuationColor!==void 0&&(i.attenuationColor=this.attenuationColor.getHex()),this.size!==void 0&&(i.size=this.size),this.sizeAttenuation!==void 0&&(i.sizeAttenuation=this.sizeAttenuation),Array.isArray(this.clippingPlanes)&&this.clippingPlanes.length>0&&(i.clippingPlanes=this.clippingPlanes.map(s=>s.toJSON())),this.rotation!==void 0&&(i.rotation=this.rotation),this.depthPacking!==void 0&&(i.depthPacking=this.depthPacking),this.linewidth!==void 0&&(i.linewidth=this.linewidth),this.linecap!==void 0&&(i.linecap=this.linecap),this.linejoin!==void 0&&(i.linejoin=this.linejoin),this.dashSize!==void 0&&(i.dashSize=this.dashSize),this.gapSize!==void 0&&(i.gapSize=this.gapSize),this.scale!==void 0&&(i.scale=this.scale),this.wireframe!==void 0&&(i.wireframe=this.wireframe),this.wireframeLinewidth!==void 0&&(i.wireframeLinewidth=this.wireframeLinewidth),this.wireframeLinecap!==void 0&&(i.wireframeLinecap=this.wireframeLinecap),this.wireframeLinejoin!==void 0&&(i.wireframeLinejoin=this.wireframeLinejoin),this.flatShading!==void 0&&(i.flatShading=this.flatShading),this.fog!==void 0&&(i.fog=this.fog),Object.keys(this.userData).length>0&&(i.userData=this.userData);function r(s){let o=[];for(let a in s){let l=s[a];delete l.metadata,o.push(l)}return o}if(t){let s=r(e.textures),o=r(e.images);s.length>0&&(i.textures=s),o.length>0&&(i.images=o)}return i}fromJSON(e,t){if(e.uuid!==void 0&&(this.uuid=e.uuid),e.name!==void 0&&(this.name=e.name),e.color!==void 0&&this.color!==void 0&&this.color.setHex(e.color),e.roughness!==void 0&&(this.roughness=e.roughness),e.metalness!==void 0&&(this.metalness=e.metalness),e.sheen!==void 0&&(this.sheen=e.sheen),e.sheenColor!==void 0&&(this.sheenColor=new ke().setHex(e.sheenColor)),e.sheenRoughness!==void 0&&(this.sheenRoughness=e.sheenRoughness),e.emissive!==void 0&&this.emissive!==void 0&&this.emissive.setHex(e.emissive),e.specular!==void 0&&this.specular!==void 0&&this.specular.setHex(e.specular),e.specularIntensity!==void 0&&(this.specularIntensity=e.specularIntensity),e.specularColor!==void 0&&this.specularColor!==void 0&&this.specularColor.setHex(e.specularColor),e.shininess!==void 0&&(this.shininess=e.shininess),e.clearcoat!==void 0&&(this.clearcoat=e.clearcoat),e.clearcoatRoughness!==void 0&&(this.clearcoatRoughness=e.clearcoatRoughness),e.dispersion!==void 0&&(this.dispersion=e.dispersion),e.retroreflectivity!==void 0&&(this.retroreflectivity=e.retroreflectivity),e.iridescence!==void 0&&(this.iridescence=e.iridescence),e.iridescenceIOR!==void 0&&(this.iridescenceIOR=e.iridescenceIOR),e.iridescenceThicknessRange!==void 0&&(this.iridescenceThicknessRange=e.iridescenceThicknessRange),e.transmission!==void 0&&(this.transmission=e.transmission),e.thickness!==void 0&&(this.thickness=e.thickness),e.attenuationDistance!==void 0&&(this.attenuationDistance=e.attenuationDistance),e.attenuationColor!==void 0&&this.attenuationColor!==void 0&&this.attenuationColor.setHex(e.attenuationColor),e.anisotropy!==void 0&&(this.anisotropy=e.anisotropy),e.anisotropyRotation!==void 0&&(this.anisotropyRotation=e.anisotropyRotation),e.fog!==void 0&&(this.fog=e.fog),e.flatShading!==void 0&&(this.flatShading=e.flatShading),e.blending!==void 0&&(this.blending=e.blending),e.combine!==void 0&&(this.combine=e.combine),e.side!==void 0&&(this.side=e.side),e.shadowSide!==void 0&&(this.shadowSide=e.shadowSide),e.opacity!==void 0&&(this.opacity=e.opacity),e.transparent!==void 0&&(this.transparent=e.transparent),e.alphaTest!==void 0&&(this.alphaTest=e.alphaTest),e.alphaHash!==void 0&&(this.alphaHash=e.alphaHash),e.depthFunc!==void 0&&(this.depthFunc=e.depthFunc),e.depthTest!==void 0&&(this.depthTest=e.depthTest),e.depthWrite!==void 0&&(this.depthWrite=e.depthWrite),e.colorWrite!==void 0&&(this.colorWrite=e.colorWrite),e.clippingPlanes!==void 0&&(this.clippingPlanes=e.clippingPlanes.map(i=>new Wn().fromJSON(i))),e.clipIntersection!==void 0&&(this.clipIntersection=e.clipIntersection),e.clipShadows!==void 0&&(this.clipShadows=e.clipShadows),e.depthPacking!==void 0&&(this.depthPacking=e.depthPacking),e.blendSrc!==void 0&&(this.blendSrc=e.blendSrc),e.blendDst!==void 0&&(this.blendDst=e.blendDst),e.blendEquation!==void 0&&(this.blendEquation=e.blendEquation),e.blendSrcAlpha!==void 0&&(this.blendSrcAlpha=e.blendSrcAlpha),e.blendDstAlpha!==void 0&&(this.blendDstAlpha=e.blendDstAlpha),e.blendEquationAlpha!==void 0&&(this.blendEquationAlpha=e.blendEquationAlpha),e.blendColor!==void 0&&this.blendColor!==void 0&&this.blendColor.setHex(e.blendColor),e.blendAlpha!==void 0&&(this.blendAlpha=e.blendAlpha),e.stencilWriteMask!==void 0&&(this.stencilWriteMask=e.stencilWriteMask),e.stencilFunc!==void 0&&(this.stencilFunc=e.stencilFunc),e.stencilRef!==void 0&&(this.stencilRef=e.stencilRef),e.stencilFuncMask!==void 0&&(this.stencilFuncMask=e.stencilFuncMask),e.stencilFail!==void 0&&(this.stencilFail=e.stencilFail),e.stencilZFail!==void 0&&(this.stencilZFail=e.stencilZFail),e.stencilZPass!==void 0&&(this.stencilZPass=e.stencilZPass),e.stencilWrite!==void 0&&(this.stencilWrite=e.stencilWrite),e.wireframe!==void 0&&(this.wireframe=e.wireframe),e.wireframeLinewidth!==void 0&&(this.wireframeLinewidth=e.wireframeLinewidth),e.wireframeLinecap!==void 0&&(this.wireframeLinecap=e.wireframeLinecap),e.wireframeLinejoin!==void 0&&(this.wireframeLinejoin=e.wireframeLinejoin),e.rotation!==void 0&&(this.rotation=e.rotation),e.linewidth!==void 0&&(this.linewidth=e.linewidth),e.linecap!==void 0&&(this.linecap=e.linecap),e.linejoin!==void 0&&(this.linejoin=e.linejoin),e.dashSize!==void 0&&(this.dashSize=e.dashSize),e.gapSize!==void 0&&(this.gapSize=e.gapSize),e.scale!==void 0&&(this.scale=e.scale),e.polygonOffset!==void 0&&(this.polygonOffset=e.polygonOffset),e.polygonOffsetFactor!==void 0&&(this.polygonOffsetFactor=e.polygonOffsetFactor),e.polygonOffsetUnits!==void 0&&(this.polygonOffsetUnits=e.polygonOffsetUnits),e.dithering!==void 0&&(this.dithering=e.dithering),e.alphaToCoverage!==void 0&&(this.alphaToCoverage=e.alphaToCoverage),e.premultipliedAlpha!==void 0&&(this.premultipliedAlpha=e.premultipliedAlpha),e.forceSinglePass!==void 0&&(this.forceSinglePass=e.forceSinglePass),e.allowOverride!==void 0&&(this.allowOverride=e.allowOverride),e.visible!==void 0&&(this.visible=e.visible),e.toneMapped!==void 0&&(this.toneMapped=e.toneMapped),e.userData!==void 0&&(this.userData=e.userData),e.vertexColors!==void 0&&(typeof e.vertexColors=="number"?this.vertexColors=e.vertexColors>0:this.vertexColors=e.vertexColors),e.size!==void 0&&(this.size=e.size),e.sizeAttenuation!==void 0&&(this.sizeAttenuation=e.sizeAttenuation),e.map!==void 0&&(this.map=t[e.map]||null),e.matcap!==void 0&&(this.matcap=t[e.matcap]||null),e.alphaMap!==void 0&&(this.alphaMap=t[e.alphaMap]||null),e.bumpMap!==void 0&&(this.bumpMap=t[e.bumpMap]||null),e.bumpScale!==void 0&&(this.bumpScale=e.bumpScale),e.normalMap!==void 0&&(this.normalMap=t[e.normalMap]||null),e.normalMapType!==void 0&&(this.normalMapType=e.normalMapType),e.normalScale!==void 0){let i=e.normalScale;Array.isArray(i)===!1&&(i=[i,i]),this.normalScale=new Xe().fromArray(i)}return e.displacementMap!==void 0&&(this.displacementMap=t[e.displacementMap]||null),e.displacementScale!==void 0&&(this.displacementScale=e.displacementScale),e.displacementBias!==void 0&&(this.displacementBias=e.displacementBias),e.roughnessMap!==void 0&&(this.roughnessMap=t[e.roughnessMap]||null),e.metalnessMap!==void 0&&(this.metalnessMap=t[e.metalnessMap]||null),e.emissiveMap!==void 0&&(this.emissiveMap=t[e.emissiveMap]||null),e.emissiveIntensity!==void 0&&(this.emissiveIntensity=e.emissiveIntensity),e.specularMap!==void 0&&(this.specularMap=t[e.specularMap]||null),e.specularIntensityMap!==void 0&&(this.specularIntensityMap=t[e.specularIntensityMap]||null),e.specularColorMap!==void 0&&(this.specularColorMap=t[e.specularColorMap]||null),e.envMap!==void 0&&(this.envMap=t[e.envMap]||null),e.envMapRotation!==void 0&&this.envMapRotation.fromArray(e.envMapRotation),e.envMapIntensity!==void 0&&(this.envMapIntensity=e.envMapIntensity),e.reflectivity!==void 0&&(this.reflectivity=e.reflectivity),e.refractionRatio!==void 0&&(this.refractionRatio=e.refractionRatio),e.lightMap!==void 0&&(this.lightMap=t[e.lightMap]||null),e.lightMapIntensity!==void 0&&(this.lightMapIntensity=e.lightMapIntensity),e.aoMap!==void 0&&(this.aoMap=t[e.aoMap]||null),e.aoMapIntensity!==void 0&&(this.aoMapIntensity=e.aoMapIntensity),e.gradientMap!==void 0&&(this.gradientMap=t[e.gradientMap]||null),e.clearcoatMap!==void 0&&(this.clearcoatMap=t[e.clearcoatMap]||null),e.clearcoatRoughnessMap!==void 0&&(this.clearcoatRoughnessMap=t[e.clearcoatRoughnessMap]||null),e.clearcoatNormalMap!==void 0&&(this.clearcoatNormalMap=t[e.clearcoatNormalMap]||null),e.clearcoatNormalScale!==void 0&&(this.clearcoatNormalScale=new Xe().fromArray(e.clearcoatNormalScale)),e.iridescenceMap!==void 0&&(this.iridescenceMap=t[e.iridescenceMap]||null),e.iridescenceThicknessMap!==void 0&&(this.iridescenceThicknessMap=t[e.iridescenceThicknessMap]||null),e.transmissionMap!==void 0&&(this.transmissionMap=t[e.transmissionMap]||null),e.thicknessMap!==void 0&&(this.thicknessMap=t[e.thicknessMap]||null),e.anisotropyMap!==void 0&&(this.anisotropyMap=t[e.anisotropyMap]||null),e.sheenColorMap!==void 0&&(this.sheenColorMap=t[e.sheenColorMap]||null),e.sheenRoughnessMap!==void 0&&(this.sheenRoughnessMap=t[e.sheenRoughnessMap]||null),this}clone(){return new this.constructor().copy(this)}copy(e){this.name=e.name,this.blending=e.blending,this.side=e.side,this.vertexColors=e.vertexColors,this.opacity=e.opacity,this.transparent=e.transparent,this.blendSrc=e.blendSrc,this.blendDst=e.blendDst,this.blendEquation=e.blendEquation,this.blendSrcAlpha=e.blendSrcAlpha,this.blendDstAlpha=e.blendDstAlpha,this.blendEquationAlpha=e.blendEquationAlpha,this.blendColor.copy(e.blendColor),this.blendAlpha=e.blendAlpha,this.depthFunc=e.depthFunc,this.depthTest=e.depthTest,this.depthWrite=e.depthWrite,this.stencilWriteMask=e.stencilWriteMask,this.stencilFunc=e.stencilFunc,this.stencilRef=e.stencilRef,this.stencilFuncMask=e.stencilFuncMask,this.stencilFail=e.stencilFail,this.stencilZFail=e.stencilZFail,this.stencilZPass=e.stencilZPass,this.stencilWrite=e.stencilWrite;let t=e.clippingPlanes,i=null;if(t!==null){let r=t.length;i=new Array(r);for(let s=0;s!==r;++s)i[s]=t[s].clone()}return this.clippingPlanes=i,this.clipIntersection=e.clipIntersection,this.clipShadows=e.clipShadows,this.shadowSide=e.shadowSide,this.colorWrite=e.colorWrite,this.precision=e.precision,this.polygonOffset=e.polygonOffset,this.polygonOffsetFactor=e.polygonOffsetFactor,this.polygonOffsetUnits=e.polygonOffsetUnits,this.dithering=e.dithering,this.alphaTest=e.alphaTest,this.alphaHash=e.alphaHash,this.alphaToCoverage=e.alphaToCoverage,this.premultipliedAlpha=e.premultipliedAlpha,this.forceSinglePass=e.forceSinglePass,this.allowOverride=e.allowOverride,this.visible=e.visible,this.toneMapped=e.toneMapped,this.userData=JSON.parse(JSON.stringify(e.userData)),this}dispose(){this.dispatchEvent({type:"dispose"})}set needsUpdate(e){e===!0&&this.version++}};var Ri=new G,mc=new G,Ho=new G,zo=new G,Yi=class{constructor(e=new G,t=new G(0,0,-1)){this.origin=e,this.direction=t}set(e,t){return this.origin.copy(e),this.direction.copy(t),this}copy(e){return this.origin.copy(e.origin),this.direction.copy(e.direction),this}at(e,t){return t.copy(this.origin).addScaledVector(this.direction,e)}lookAt(e){return this.direction.copy(e).sub(this.origin).normalize(),this}recast(e){return this.origin.copy(this.at(e,Ri)),this}closestPointToPoint(e,t){t.subVectors(e,this.origin);let i=t.dot(this.direction);return i<0?t.copy(this.origin):t.copy(this.origin).addScaledVector(this.direction,i)}distanceToPoint(e){return Math.sqrt(this.distanceSqToPoint(e))}distanceSqToPoint(e){let t=Ri.subVectors(e,this.origin).dot(this.direction);return t<0?this.origin.distanceToSquared(e):(Ri.copy(this.origin).addScaledVector(this.direction,t),Ri.distanceToSquared(e))}distanceSqToSegment(e,t,i,r){mc.copy(e).add(t).multiplyScalar(.5),Ho.copy(t).sub(e).normalize(),zo.copy(this.origin).sub(mc);let s=e.distanceTo(t)*.5,o=-this.direction.dot(Ho),a=zo.dot(this.direction),l=-zo.dot(Ho),c=zo.lengthSq(),h=Math.abs(1-o*o),d,u,p,x;if(h>0)if(d=o*l-a,u=o*a-l,x=s*h,d>=0)if(u>=-x)if(u<=x){let v=1/h;d*=v,u*=v,p=d*(d+o*u+2*a)+u*(o*d+u+2*l)+c}else u=s,d=Math.max(0,-(o*u+a)),p=-d*d+u*(u+2*l)+c;else u=-s,d=Math.max(0,-(o*u+a)),p=-d*d+u*(u+2*l)+c;else u<=-x?(d=Math.max(0,-(-o*s+a)),u=d>0?-s:Math.min(Math.max(-s,-l),s),p=-d*d+u*(u+2*l)+c):u<=x?(d=0,u=Math.min(Math.max(-s,-l),s),p=u*(u+2*l)+c):(d=Math.max(0,-(o*s+a)),u=d>0?s:Math.min(Math.max(-s,-l),s),p=-d*d+u*(u+2*l)+c);else u=o>0?-s:s,d=Math.max(0,-(o*u+a)),p=-d*d+u*(u+2*l)+c;return i&&i.copy(this.origin).addScaledVector(this.direction,d),r&&r.copy(mc).addScaledVector(Ho,u),p}intersectSphere(e,t){if(e.radius<0)return null;Ri.subVectors(e.center,this.origin);let i=Ri.dot(this.direction),r=Ri.dot(Ri)-i*i,s=e.radius*e.radius;if(r>s)return null;let o=Math.sqrt(s-r),a=i-o,l=i+o;return l<0?null:a<0?this.at(l,t):this.at(a,t)}intersectsSphere(e){return e.radius<0?!1:this.distanceSqToPoint(e.center)<=e.radius*e.radius}distanceToPlane(e){let t=e.normal.dot(this.direction);if(t===0)return e.distanceToPoint(this.origin)===0?0:null;let i=-(this.origin.dot(e.normal)+e.constant)/t;return i>=0?i:null}intersectPlane(e,t){let i=this.distanceToPlane(e);return i===null?null:this.at(i,t)}intersectsPlane(e){let t=e.distanceToPoint(this.origin);return t===0||e.normal.dot(this.direction)*t<0}intersectBox(e,t){let i,r,s,o,a,l,c=1/this.direction.x,h=1/this.direction.y,d=1/this.direction.z,u=this.origin;return c>=0?(i=(e.min.x-u.x)*c,r=(e.max.x-u.x)*c):(i=(e.max.x-u.x)*c,r=(e.min.x-u.x)*c),h>=0?(s=(e.min.y-u.y)*h,o=(e.max.y-u.y)*h):(s=(e.max.y-u.y)*h,o=(e.min.y-u.y)*h),i>o||s>r||((s>i||isNaN(i))&&(i=s),(o<r||isNaN(r))&&(r=o),d>=0?(a=(e.min.z-u.z)*d,l=(e.max.z-u.z)*d):(a=(e.max.z-u.z)*d,l=(e.min.z-u.z)*d),i>l||a>r)||((a>i||i!==i)&&(i=a),(l<r||r!==r)&&(r=l),r<0)?null:this.at(i>=0?i:r,t)}intersectsBox(e){return this.intersectBox(e,Ri)!==null}intersectTriangle(e,t,i,r,s){let o=this.origin,a=this.direction,l=a.x,c=a.y,h=a.z,d=e.x-o.x,u=e.y-o.y,p=e.z-o.z,x=t.x-o.x,v=t.y-o.y,m=t.z-o.z,f=i.x-o.x,b=i.y-o.y,P=i.z-o.z,w=Math.abs(l),T=Math.abs(c),E=Math.abs(h),D,g,S,_,A,I,k,N,U,z,Y,ee;if(w>=T&&w>=E?(S=l,I=d,U=x,ee=f,l>=0?(D=c,g=h,_=u,A=p,k=v,N=m,z=b,Y=P):(D=h,g=c,_=p,A=u,k=m,N=v,z=P,Y=b)):T>=E?(S=c,I=u,U=v,ee=b,c>=0?(D=h,g=l,_=p,A=d,k=m,N=x,z=P,Y=f):(D=l,g=h,_=d,A=p,k=x,N=m,z=f,Y=P)):(S=h,I=p,U=m,ee=P,h>=0?(D=l,g=c,_=d,A=u,k=x,N=v,z=f,Y=b):(D=c,g=l,_=u,A=d,k=v,N=x,z=b,Y=f)),S===0)return null;let J=D/S,ie=g/S,ce=1/S,Se=_-J*I,fe=A-ie*I,Ie=k-J*U,We=N-ie*U,He=z-J*ee,oe=Y-ie*ee,le=He*We-oe*Ie,be=Se*oe-fe*He,Ve=Ie*fe-We*Se;if(r){if(le<0||be<0||Ve<0)return null}else if((le<0||be<0||Ve<0)&&(le>0||be>0||Ve>0))return null;let Me=le+be+Ve;if(Me===0)return null;let Ge=ce*(le*I+be*U+Ve*ee);return(Me>0?Ge<0:Ge>0)?null:this.at(Ge/Me,s)}applyMatrix4(e){return this.origin.applyMatrix4(e),this.direction.transformDirection(e),this}equals(e){return e.origin.equals(this.origin)&&e.direction.equals(this.direction)}clone(){return new this.constructor().copy(this)}},Ot=class extends yi{constructor(e){super(),this.isMeshBasicMaterial=!0,this.type="MeshBasicMaterial",this.color=new ke(16777215),this.map=null,this.lightMap=null,this.lightMapIntensity=1,this.aoMap=null,this.aoMapIntensity=1,this.specularMap=null,this.alphaMap=null,this.envMap=null,this.envMapRotation=new $i,this.combine=Dc,this.reflectivity=1,this.refractionRatio=.98,this.wireframe=!1,this.wireframeLinewidth=1,this.wireframeLinecap="round",this.wireframeLinejoin="round",this.fog=!0,this.setValues(e)}copy(e){return super.copy(e),this.color.copy(e.color),this.map=e.map,this.lightMap=e.lightMap,this.lightMapIntensity=e.lightMapIntensity,this.aoMap=e.aoMap,this.aoMapIntensity=e.aoMapIntensity,this.specularMap=e.specularMap,this.alphaMap=e.alphaMap,this.envMap=e.envMap,this.envMapRotation.copy(e.envMapRotation),this.combine=e.combine,this.reflectivity=e.reflectivity,this.refractionRatio=e.refractionRatio,this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this.wireframeLinecap=e.wireframeLinecap,this.wireframeLinejoin=e.wireframeLinejoin,this.fog=e.fog,this}},fh=new wt,fr=new Yi,Go=new Pi,ph=new G,Vo=new G,Wo=new G,Xo=new G,gc=new G,qo=new G,mh=new G,$o=new G,$e=class extends yn{constructor(e=new Yt,t=new Ot){super(),this.isMesh=!0,this.type="Mesh",this.geometry=e,this.material=t,this.morphTargetDictionary=void 0,this.morphTargetInfluences=void 0,this.count=1,this.updateMorphTargets()}copy(e,t){return super.copy(e,t),e.morphTargetInfluences!==void 0&&(this.morphTargetInfluences=e.morphTargetInfluences.slice()),e.morphTargetDictionary!==void 0&&(this.morphTargetDictionary=Object.assign({},e.morphTargetDictionary)),this.material=Array.isArray(e.material)?e.material.slice():e.material,this.geometry=e.geometry,this}updateMorphTargets(){let t=this.geometry.morphAttributes,i=Object.keys(t);if(i.length>0){let r=t[i[0]];if(r!==void 0){this.morphTargetInfluences=[],this.morphTargetDictionary={};for(let s=0,o=r.length;s<o;s++){let a=r[s].name||String(s);this.morphTargetInfluences.push(0),this.morphTargetDictionary[a]=s}}}}getVertexPosition(e,t){let i=this.geometry,r=i.attributes.position,s=i.morphAttributes.position,o=i.morphTargetsRelative;t.fromBufferAttribute(r,e);let a=this.morphTargetInfluences;if(s&&a){qo.set(0,0,0);for(let l=0,c=s.length;l<c;l++){let h=a[l],d=s[l];h!==0&&(gc.fromBufferAttribute(d,e),o?qo.addScaledVector(gc,h):qo.addScaledVector(gc.sub(t),h))}t.add(qo)}return t}intersectsFrustum(e){return e.intersectsObject(this)}raycast(e,t){let i=this.geometry,r=this.material,s=this.matrixWorld;r!==void 0&&(i.boundingSphere===null&&i.computeBoundingSphere(),Go.copy(i.boundingSphere),Go.applyMatrix4(s),fr.copy(e.ray).recast(e.near),!(Go.containsPoint(fr.origin)===!1&&(fr.intersectSphere(Go,ph)===null||fr.origin.distanceToSquared(ph)>(e.far-e.near)**2))&&(fh.copy(s).invert(),fr.copy(e.ray).applyMatrix4(fh),!(i.boundingBox!==null&&fr.intersectsBox(i.boundingBox)===!1)&&this._computeIntersections(e,t,fr)))}_computeIntersections(e,t,i){let r,s=this.geometry,o=this.material,a=s.index,l=s.attributes.position,c=s.attributes.uv,h=s.attributes.uv1,d=s.attributes.normal,u=s.groups,p=s.drawRange;if(a!==null)if(Array.isArray(o))for(let x=0,v=u.length;x<v;x++){let m=u[x],f=o[m.materialIndex],b=Math.max(m.start,p.start),P=Math.min(a.count,Math.min(m.start+m.count,p.start+p.count));for(let w=b,T=P;w<T;w+=3){let E=a.getX(w),D=a.getX(w+1),g=a.getX(w+2);r=Yo(this,f,e,i,c,h,d,E,D,g),r&&(r.faceIndex=Math.floor(w/3),r.face.materialIndex=m.materialIndex,t.push(r))}}else{let x=Math.max(0,p.start),v=Math.min(a.count,p.start+p.count);for(let m=x,f=v;m<f;m+=3){let b=a.getX(m),P=a.getX(m+1),w=a.getX(m+2);r=Yo(this,o,e,i,c,h,d,b,P,w),r&&(r.faceIndex=Math.floor(m/3),t.push(r))}}else if(l!==void 0)if(Array.isArray(o))for(let x=0,v=u.length;x<v;x++){let m=u[x],f=o[m.materialIndex],b=Math.max(m.start,p.start),P=Math.min(l.count,Math.min(m.start+m.count,p.start+p.count));for(let w=b,T=P;w<T;w+=3){let E=w,D=w+1,g=w+2;r=Yo(this,f,e,i,c,h,d,E,D,g),r&&(r.faceIndex=Math.floor(w/3),r.face.materialIndex=m.materialIndex,t.push(r))}}else{let x=Math.max(0,p.start),v=Math.min(l.count,p.start+p.count);for(let m=x,f=v;m<f;m+=3){let b=m,P=m+1,w=m+2;r=Yo(this,o,e,i,c,h,d,b,P,w),r&&(r.faceIndex=Math.floor(m/3),t.push(r))}}}};function nm(n,e,t,i,r,s,o,a){let l;if(e.side===mn?l=i.intersectTriangle(o,s,r,!0,a):l=i.intersectTriangle(r,s,o,e.side===tr,a),l===null)return null;$o.copy(a),$o.applyMatrix4(n.matrixWorld);let c=t.ray.origin.distanceTo($o);return c<t.near||c>t.far?null:{distance:c,point:$o.clone(),object:n}}function Yo(n,e,t,i,r,s,o,a,l,c){n.getVertexPosition(a,Vo),n.getVertexPosition(l,Wo),n.getVertexPosition(c,Xo);let h=nm(n,e,t,i,Vo,Wo,Xo,mh);if(h){let d=new G;qi.getBarycoord(mh,Vo,Wo,Xo,d),r&&(h.uv=qi.getInterpolatedAttribute(r,a,l,c,d,new Xe)),s&&(h.uv1=qi.getInterpolatedAttribute(s,a,l,c,d,new Xe)),o&&(h.normal=qi.getInterpolatedAttribute(o,a,l,c,d,new G),h.normal.dot(i.direction)>0&&h.normal.multiplyScalar(-1));let u={a,b:l,c,normal:new G,materialIndex:0};qi.getNormal(Vo,Wo,Xo,u.normal),h.face=u,h.barycoord=d}return h}var vr=class extends Pn{constructor(e=null,t=1,i=1,r,s,o,a,l,c=en,h=en,d,u){super(null,o,a,l,c,h,r,s,d,u),this.isDataTexture=!0,this.image={data:e,width:t,height:i},this.generateMipmaps=!1,this.flipY=!1,this.unpackAlignment=1}};var Xs=class extends Cn{constructor(e,t,i,r=1){super(e,t,i),this.isInstancedBufferAttribute=!0,this.meshPerAttribute=r}copy(e){return super.copy(e),this.meshPerAttribute=e.meshPerAttribute,this}toJSON(){let e=super.toJSON();return e.meshPerAttribute=this.meshPerAttribute,e.isInstancedBufferAttribute=!0,e}},$r=new wt,gh=new wt,Zo=[],xh=new Nn,im=new wt,Ls=new $e,Ds=new Pi,qs=class extends $e{constructor(e,t,i){super(e,t),this.isInstancedMesh=!0,this.instanceMatrix=new Xs(new Float32Array(i*16),16),this.instanceColor=null,this.morphTexture=null,this.count=i,this.boundingBox=null,this.boundingSphere=null;for(let r=0;r<i;r++)this.setMatrixAt(r,im)}computeBoundingBox(){let e=this.geometry,t=this.count;this.boundingBox===null&&(this.boundingBox=new Nn),e.boundingBox===null&&e.computeBoundingBox(),this.boundingBox.makeEmpty();for(let i=0;i<t;i++)this.getMatrixAt(i,$r),xh.copy(e.boundingBox).applyMatrix4($r),this.boundingBox.union(xh)}computeBoundingSphere(){let e=this.geometry,t=this.count;this.boundingSphere===null&&(this.boundingSphere=new Pi),e.boundingSphere===null&&e.computeBoundingSphere(),this.boundingSphere.makeEmpty();for(let i=0;i<t;i++)this.getMatrixAt(i,$r),Ds.copy(e.boundingSphere).applyMatrix4($r),this.boundingSphere.union(Ds)}copy(e,t){return super.copy(e,t),this.instanceMatrix.copy(e.instanceMatrix),e.morphTexture!==null&&(this.morphTexture=e.morphTexture.clone()),e.instanceColor!==null&&(this.instanceColor=e.instanceColor.clone()),this.count=e.count,e.boundingBox!==null&&(this.boundingBox=e.boundingBox.clone()),e.boundingSphere!==null&&(this.boundingSphere=e.boundingSphere.clone()),this}getColorAt(e,t){return this.instanceColor===null?t.setRGB(1,1,1):t.fromArray(this.instanceColor.array,e*3)}getMatrixAt(e,t){return t.fromArray(this.instanceMatrix.array,e*16)}getMorphAt(e,t){let i=t.morphTargetInfluences,r=this.morphTexture.source.data.data,s=i.length+1,o=e*s+1;for(let a=0;a<i.length;a++)i[a]=r[o+a]}raycast(e,t){let i=this.matrixWorld,r=this.count;if(Ls.geometry=this.geometry,Ls.material=this.material,Ls.material!==void 0&&(this.boundingSphere===null&&this.computeBoundingSphere(),Ds.copy(this.boundingSphere),Ds.applyMatrix4(i),e.ray.intersectsSphere(Ds)!==!1))for(let s=0;s<r;s++){this.getMatrixAt(s,$r),gh.multiplyMatrices(i,$r),Ls.matrixWorld=gh,Ls.raycast(e,Zo);for(let o=0,a=Zo.length;o<a;o++){let l=Zo[o];l.instanceId=s,l.object=this,t.push(l)}Zo.length=0}}setColorAt(e,t){return this.instanceColor===null&&(this.instanceColor=new Xs(new Float32Array(this.instanceMatrix.count*3).fill(1),3)),t.toArray(this.instanceColor.array,e*3),this}setMatrixAt(e,t){return t.toArray(this.instanceMatrix.array,e*16),this}setMorphAt(e,t){let i=t.morphTargetInfluences,r=i.length+1;this.morphTexture===null&&(this.morphTexture=new vr(new Float32Array(r*this.count),r,this.count,Oa,Jn));let s=this.morphTexture.source.data.data,o=0;for(let c=0;c<i.length;c++)o+=i[c];let a=this.geometry.morphTargetsRelative?1:1-o,l=r*e;return s[l]=a,s.set(i,l+1),this}updateMorphTargets(){}dispose(){super.dispose(),this.morphTexture!==null&&(this.morphTexture.dispose(),this.morphTexture=null)}},pr=new Pi,rm=new Xe(.5,.5),Ko=new G,os=class{constructor(e=new Wn,t=new Wn,i=new Wn,r=new Wn,s=new Wn,o=new Wn){this.planes=[e,t,i,r,s,o]}set(e,t,i,r,s,o){let a=this.planes;return a[0].copy(e),a[1].copy(t),a[2].copy(i),a[3].copy(r),a[4].copy(s),a[5].copy(o),this}copy(e){let t=this.planes;for(let i=0;i<6;i++)t[i].copy(e.planes[i]);return this}setFromProjectionMatrix(e,t=ai,i=!1){let r=this.planes,s=e.elements,o=s[0],a=s[1],l=s[2],c=s[3],h=s[4],d=s[5],u=s[6],p=s[7],x=s[8],v=s[9],m=s[10],f=s[11],b=s[12],P=s[13],w=s[14],T=s[15];if(r[0].setComponents(c-o,p-h,f-x,T-b).normalize(),r[1].setComponents(c+o,p+h,f+x,T+b).normalize(),r[2].setComponents(c+a,p+d,f+v,T+P).normalize(),r[3].setComponents(c-a,p-d,f-v,T-P).normalize(),i)r[4].setComponents(l,u,m,w).normalize(),r[5].setComponents(c-l,p-u,f-m,T-w).normalize();else if(r[4].setComponents(c-l,p-u,f-m,T-w).normalize(),t===ai)r[5].setComponents(c+l,p+u,f+m,T+w).normalize();else if(t===es)r[5].setComponents(l,u,m,w).normalize();else throw new Error("THREE.Frustum.setFromProjectionMatrix(): Invalid coordinate system: "+t);return this}intersectsObject(e){if(e.boundingSphere!==void 0)e.boundingSphere===null&&e.computeBoundingSphere(),pr.copy(e.boundingSphere).applyMatrix4(e.matrixWorld);else{let t=e.geometry;t.boundingSphere===null&&t.computeBoundingSphere(),pr.copy(t.boundingSphere).applyMatrix4(e.matrixWorld)}return this.intersectsSphere(pr)}intersectsSprite(e){pr.center.set(0,0,0);let t=rm.distanceTo(e.center);return pr.radius=.7071067811865476+t,pr.applyMatrix4(e.matrixWorld),this.intersectsSphere(pr)}intersectsSphere(e){let t=this.planes,i=e.center,r=-e.radius;for(let s=0;s<6;s++)if(t[s].distanceToPoint(i)<r)return!1;return!0}intersectsBox(e){let t=this.planes;for(let i=0;i<6;i++){let r=t[i];if(Ko.x=r.normal.x>0?e.max.x:e.min.x,Ko.y=r.normal.y>0?e.max.y:e.min.y,Ko.z=r.normal.z>0?e.max.z:e.min.z,r.distanceToPoint(Ko)<0)return!1}return!0}containsPoint(e){let t=this.planes;for(let i=0;i<6;i++)if(t[i].distanceToPoint(e)<0)return!1;return!0}clone(){return new this.constructor().copy(this)}};var as=class extends yi{constructor(e){super(),this.isPointsMaterial=!0,this.type="PointsMaterial",this.color=new ke(16777215),this.map=null,this.alphaMap=null,this.size=1,this.sizeAttenuation=!0,this.fog=!0,this.setValues(e)}copy(e){return super.copy(e),this.color.copy(e.color),this.map=e.map,this.alphaMap=e.alphaMap,this.size=e.size,this.sizeAttenuation=e.sizeAttenuation,this.fog=e.fog,this}},vh=new wt,Sc=new Yi,Jo=new Pi,jo=new G,$s=class extends yn{constructor(e=new Yt,t=new as){super(),this.isPoints=!0,this.type="Points",this.geometry=e,this.material=t,this.morphTargetDictionary=void 0,this.morphTargetInfluences=void 0,this.updateMorphTargets()}copy(e,t){return super.copy(e,t),this.material=Array.isArray(e.material)?e.material.slice():e.material,this.geometry=e.geometry,this}intersectsFrustum(e){return e.intersectsObject(this)}raycast(e,t){let i=this.geometry,r=this.matrixWorld,s=e.params.Points.threshold,o=i.drawRange;if(i.boundingSphere===null&&i.computeBoundingSphere(),Jo.copy(i.boundingSphere),Jo.applyMatrix4(r),Jo.radius+=s,e.ray.intersectsSphere(Jo)===!1)return;vh.copy(r).invert(),Sc.copy(e.ray).applyMatrix4(vh);let a=s/((this.scale.x+this.scale.y+this.scale.z)/3),l=a*a,c=i.index,d=i.attributes.position;if(c!==null){let u=Math.max(0,o.start),p=Math.min(c.count,o.start+o.count);for(let x=u,v=p;x<v;x++){let m=c.getX(x);jo.fromBufferAttribute(d,m),yh(jo,m,l,r,e,t,this)}}else{let u=Math.max(0,o.start),p=Math.min(d.count,o.start+o.count);for(let x=u,v=p;x<v;x++)jo.fromBufferAttribute(d,x),yh(jo,x,l,r,e,t,this)}}updateMorphTargets(){let t=this.geometry.morphAttributes,i=Object.keys(t);if(i.length>0){let r=t[i[0]];if(r!==void 0){this.morphTargetInfluences=[],this.morphTargetDictionary={};for(let s=0,o=r.length;s<o;s++){let a=r[s].name||String(s);this.morphTargetInfluences.push(0),this.morphTargetDictionary[a]=s}}}}};function yh(n,e,t,i,r,s,o){let a=Sc.distanceSqToPoint(n);if(a<t){let l=new G;Sc.closestPointToPoint(n,l),l.applyMatrix4(i);let c=r.ray.origin.distanceTo(l);if(c<r.near||c>r.far)return;s.push({distance:c,distanceToRay:Math.sqrt(a),point:l,index:e,face:null,faceIndex:null,barycoord:null,object:o})}}var Ys=class extends Pn{constructor(e=[],t=nr,i,r,s,o,a,l,c,h){super(e,t,i,r,s,o,a,l,c,h),this.isCubeTexture=!0,this.flipY=!1}get images(){return this.image}set images(e){this.image=e}},Ii=class extends Pn{constructor(e,t,i,r,s,o,a,l,c){super(e,t,i,r,s,o,a,l,c),this.isCanvasTexture=!0,this.needsUpdate=!0}};var Zi=class extends Pn{constructor(e,t,i=ui,r,s,o,a=en,l=en,c,h=vi,d=1){if(h!==vi&&h!==rr)throw new Error("THREE.DepthTexture: format must be either THREE.DepthFormat or THREE.DepthStencilFormat");let u={width:e,height:t,depth:d};super(u,r,s,o,a,l,h,i,c),this.isDepthTexture=!0,this.flipY=!1,this.generateMipmaps=!1,this.compareFunction=null}copy(e){return super.copy(e),this.source=new is(Object.assign({},e.image)),this.compareFunction=e.compareFunction,this}toJSON(e){let t=super.toJSON(e);return t.compareFunction=this.compareFunction,t}},ma=class extends Zi{constructor(e,t=ui,i=nr,r,s,o=en,a=en,l,c=vi){let h={width:e,height:e,depth:1},d=[h,h,h,h,h,h];super(e,e,t,i,r,s,o,a,l,c),this.image=d,this.isCubeDepthTexture=!0,this.isCubeTexture=!0}get images(){return this.image}set images(e){this.image=e}},Zs=class extends Pn{constructor(e=null){super(),this.sourceTexture=e,this.isExternalTexture=!0}copy(e){return super.copy(e),this.sourceTexture=e.sourceTexture,this}},bt=class n extends Yt{constructor(e=1,t=1,i=1,r=1,s=1,o=1){super(),this.type="BoxGeometry",this.parameters={width:e,height:t,depth:i,widthSegments:r,heightSegments:s,depthSegments:o};let a=this;r=Math.floor(r),s=Math.floor(s),o=Math.floor(o);let l=[],c=[],h=[],d=[],u=0,p=0;x("z","y","x",-1,-1,i,t,e,o,s,0),x("z","y","x",1,-1,i,t,-e,o,s,1),x("x","z","y",1,1,e,i,t,r,o,2),x("x","z","y",1,-1,e,i,-t,r,o,3),x("x","y","z",1,-1,e,t,i,r,s,4),x("x","y","z",-1,-1,e,t,-i,r,s,5),this.setIndex(l),this.setAttribute("position",new _t(c,3)),this.setAttribute("normal",new _t(h,3)),this.setAttribute("uv",new _t(d,2));function x(v,m,f,b,P,w,T,E,D,g,S){let _=w/D,A=T/g,I=w/2,k=T/2,N=E/2,U=D+1,z=g+1,Y=0,ee=0,J=new G;for(let ie=0;ie<z;ie++){let ce=ie*A-k;for(let Se=0;Se<U;Se++){let fe=Se*_-I;J[v]=fe*b,J[m]=ce*P,J[f]=N,c.push(J.x,J.y,J.z),J[v]=0,J[m]=0,J[f]=E>0?1:-1,h.push(J.x,J.y,J.z),d.push(Se/D),d.push(1-ie/g),Y+=1}}for(let ie=0;ie<g;ie++)for(let ce=0;ce<D;ce++){let Se=u+ce+U*ie,fe=u+ce+U*(ie+1),Ie=u+(ce+1)+U*(ie+1),We=u+(ce+1)+U*ie;l.push(Se,fe,We),l.push(fe,Ie,We),ee+=6}a.addGroup(p,ee,S),p+=ee,u+=Y}}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.width,e.height,e.depth,e.widthSegments,e.heightSegments,e.depthSegments)}};var In=class n extends Yt{constructor(e=1,t=32,i=0,r=Math.PI*2){super(),this.type="CircleGeometry",this.parameters={radius:e,segments:t,thetaStart:i,thetaLength:r},t=Math.max(3,t);let s=[],o=[],a=[],l=[],c=new G,h=new Xe;o.push(0,0,0),a.push(0,0,1),l.push(.5,.5);for(let d=0,u=3;d<=t;d++,u+=3){let p=i+d/t*r;c.x=e*Math.cos(p),c.y=e*Math.sin(p),o.push(c.x,c.y,c.z),a.push(0,0,1),h.x=(o[u]/e+1)/2,h.y=(o[u+1]/e+1)/2,l.push(h.x,h.y)}for(let d=1;d<=t;d++)s.push(d,d+1,0);this.setIndex(s),this.setAttribute("position",new _t(o,3)),this.setAttribute("normal",new _t(a,3)),this.setAttribute("uv",new _t(l,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radius,e.segments,e.thetaStart,e.thetaLength)}},Wt=class n extends Yt{constructor(e=1,t=1,i=1,r=32,s=1,o=!1,a=0,l=Math.PI*2){super(),this.type="CylinderGeometry",this.parameters={radiusTop:e,radiusBottom:t,height:i,radialSegments:r,heightSegments:s,openEnded:o,thetaStart:a,thetaLength:l};let c=this;r=Math.floor(r),s=Math.floor(s);let h=[],d=[],u=[],p=[],x=0,v=[],m=i/2,f=0;b(),o===!1&&(e>0&&P(!0),t>0&&P(!1)),this.setIndex(h),this.setAttribute("position",new _t(d,3)),this.setAttribute("normal",new _t(u,3)),this.setAttribute("uv",new _t(p,2));function b(){let w=new G,T=new G,E=0,D=(t-e)/i;for(let g=0;g<=s;g++){let S=[],_=g/s,A=_*(t-e)+e;for(let I=0;I<=r;I++){let k=I/r,N=k*l+a,U=Math.sin(N),z=Math.cos(N);T.x=A*U,T.y=-_*i+m,T.z=A*z,d.push(T.x,T.y,T.z),w.set(U,D,z).normalize(),u.push(w.x,w.y,w.z),p.push(k,1-_),S.push(x++)}v.push(S)}for(let g=0;g<r;g++)for(let S=0;S<s;S++){let _=v[S][g],A=v[S+1][g],I=v[S+1][g+1],k=v[S][g+1];(e>0||S!==0)&&(h.push(_,A,k),E+=3),(t>0||S!==s-1)&&(h.push(A,I,k),E+=3)}c.addGroup(f,E,0),f+=E}function P(w){let T=x,E=new Xe,D=new G,g=0,S=w===!0?e:t,_=w===!0?1:-1;for(let I=1;I<=r;I++)d.push(0,m*_,0),u.push(0,_,0),p.push(.5,.5),x++;let A=x;for(let I=0;I<=r;I++){let N=I/r*l+a,U=Math.cos(N),z=Math.sin(N);D.x=S*z,D.y=m*_,D.z=S*U,d.push(D.x,D.y,D.z),u.push(0,_,0),E.x=U*.5+.5,E.y=z*.5*_+.5,p.push(E.x,E.y),x++}for(let I=0;I<r;I++){let k=T+I,N=A+I;w===!0?h.push(N,N+1,k):h.push(N+1,N,k),g+=3}c.addGroup(f,g,w===!0?1:2),f+=g}}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radiusTop,e.radiusBottom,e.height,e.radialSegments,e.heightSegments,e.openEnded,e.thetaStart,e.thetaLength)}},yr=class n extends Wt{constructor(e=1,t=1,i=32,r=1,s=!1,o=0,a=Math.PI*2){super(0,e,t,i,r,s,o,a),this.type="ConeGeometry",this.parameters={radius:e,height:t,radialSegments:i,heightSegments:r,openEnded:s,thetaStart:o,thetaLength:a}}static fromJSON(e){return new n(e.radius,e.height,e.radialSegments,e.heightSegments,e.openEnded,e.thetaStart,e.thetaLength)}},ga=class n extends Yt{constructor(e=[],t=[],i=1,r=0){super(),this.type="PolyhedronGeometry",this.parameters={vertices:e,indices:t,radius:i,detail:r};let s=[],o=[];a(r),c(i),h(),this.setAttribute("position",new _t(s,3)),this.setAttribute("normal",new _t(s.slice(),3)),this.setAttribute("uv",new _t(o,2)),r===0?this.computeVertexNormals():this.normalizeNormals();function a(b){let P=new G,w=new G,T=new G;for(let E=0;E<t.length;E+=3)p(t[E+0],P),p(t[E+1],w),p(t[E+2],T),l(P,w,T,b)}function l(b,P,w,T){let E=T+1,D=[];for(let g=0;g<=E;g++){D[g]=[];let S=b.clone().lerp(w,g/E),_=P.clone().lerp(w,g/E),A=E-g;for(let I=0;I<=A;I++)I===0&&g===E?D[g][I]=S:D[g][I]=S.clone().lerp(_,I/A)}for(let g=0;g<E;g++)for(let S=0;S<2*(E-g)-1;S++){let _=Math.floor(S/2);S%2===0?(u(D[g][_+1]),u(D[g+1][_]),u(D[g][_])):(u(D[g][_+1]),u(D[g+1][_+1]),u(D[g+1][_]))}}function c(b){let P=new G;for(let w=0;w<s.length;w+=3)P.x=s[w+0],P.y=s[w+1],P.z=s[w+2],P.normalize().multiplyScalar(b),s[w+0]=P.x,s[w+1]=P.y,s[w+2]=P.z}function h(){let b=new G;for(let P=0;P<s.length;P+=3){b.x=s[P+0],b.y=s[P+1],b.z=s[P+2];let w=m(b)/2/Math.PI+.5,T=f(b)/Math.PI+.5;o.push(w,1-T)}x(),d()}function d(){for(let b=0;b<o.length;b+=6){let P=o[b+0],w=o[b+2],T=o[b+4],E=Math.max(P,w,T),D=Math.min(P,w,T);E>.9&&D<.1&&(P<.2&&(o[b+0]+=1),w<.2&&(o[b+2]+=1),T<.2&&(o[b+4]+=1))}}function u(b){s.push(b.x,b.y,b.z)}function p(b,P){let w=b*3;P.x=e[w+0],P.y=e[w+1],P.z=e[w+2]}function x(){let b=new G,P=new G,w=new G,T=new G,E=new Xe,D=new Xe,g=new Xe;for(let S=0,_=0;S<s.length;S+=9,_+=6){b.set(s[S+0],s[S+1],s[S+2]),P.set(s[S+3],s[S+4],s[S+5]),w.set(s[S+6],s[S+7],s[S+8]),E.set(o[_+0],o[_+1]),D.set(o[_+2],o[_+3]),g.set(o[_+4],o[_+5]),T.copy(b).add(P).add(w).divideScalar(3);let A=m(T);v(E,_+0,b,A),v(D,_+2,P,A),v(g,_+4,w,A)}}function v(b,P,w,T){T<0&&b.x===1&&(o[P]=b.x-1),w.x===0&&w.z===0&&(o[P]=T/2/Math.PI+.5)}function m(b){return Math.atan2(b.z,-b.x)}function f(b){return Math.atan2(-b.y,Math.sqrt(b.x*b.x+b.z*b.z))}}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.vertices,e.indices,e.radius,e.detail)}},Ks=class n extends ga{constructor(e=1,t=0){let i=(1+Math.sqrt(5))/2,r=1/i,s=[-1,-1,-1,-1,-1,1,-1,1,-1,-1,1,1,1,-1,-1,1,-1,1,1,1,-1,1,1,1,0,-r,-i,0,-r,i,0,r,-i,0,r,i,-r,-i,0,-r,i,0,r,-i,0,r,i,0,-i,0,-r,i,0,-r,-i,0,r,i,0,r],o=[3,11,7,3,7,15,3,15,13,7,19,17,7,17,6,7,6,15,17,4,8,17,8,10,17,10,6,8,0,16,8,16,2,8,2,10,0,12,1,0,1,18,0,18,16,6,10,2,6,2,13,6,13,15,2,16,18,2,18,3,2,3,13,18,1,9,18,9,11,18,11,3,4,14,12,4,12,0,4,0,8,11,9,5,11,5,19,11,19,7,19,5,14,19,14,4,19,4,17,1,12,14,1,14,5,1,5,9];super(s,o,e,t),this.type="DodecahedronGeometry",this.parameters={radius:e,detail:t}}static fromJSON(e){return new n(e.radius,e.detail)}};var Xn=class n extends Yt{constructor(e=1,t=1,i=1,r=1){super(),this.type="PlaneGeometry",this.parameters={width:e,height:t,widthSegments:i,heightSegments:r};let s=e/2,o=t/2,a=Math.floor(i),l=Math.floor(r),c=a+1,h=l+1,d=e/a,u=t/l,p=[],x=[],v=[],m=[];for(let f=0;f<h;f++){let b=f*u-o;for(let P=0;P<c;P++){let w=P*d-s;x.push(w,-b,0),v.push(0,0,1),m.push(P/a),m.push(1-f/l)}}for(let f=0;f<l;f++)for(let b=0;b<a;b++){let P=b+c*f,w=b+c*(f+1),T=b+1+c*(f+1),E=b+1+c*f;p.push(P,w,E),p.push(w,T,E)}this.setIndex(p),this.setAttribute("position",new _t(x,3)),this.setAttribute("normal",new _t(v,3)),this.setAttribute("uv",new _t(m,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.width,e.height,e.widthSegments,e.heightSegments)}},Js=class n extends Yt{constructor(e=.5,t=1,i=32,r=1,s=0,o=Math.PI*2){super(),this.type="RingGeometry",this.parameters={innerRadius:e,outerRadius:t,thetaSegments:i,phiSegments:r,thetaStart:s,thetaLength:o},i=Math.max(3,i),r=Math.max(1,r);let a=[],l=[],c=[],h=[],d=e,u=(t-e)/r,p=new G,x=new Xe;for(let v=0;v<=r;v++){for(let m=0;m<=i;m++){let f=s+m/i*o;p.x=d*Math.cos(f),p.y=d*Math.sin(f),l.push(p.x,p.y,p.z),c.push(0,0,1),x.x=(p.x/t+1)/2,x.y=(p.y/t+1)/2,h.push(x.x,x.y)}d+=u}for(let v=0;v<r;v++){let m=v*(i+1);for(let f=0;f<i;f++){let b=f+m,P=b,w=b+i+1,T=b+i+2,E=b+1;a.push(P,w,E),a.push(w,T,E)}}this.setIndex(a),this.setAttribute("position",new _t(l,3)),this.setAttribute("normal",new _t(c,3)),this.setAttribute("uv",new _t(h,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.innerRadius,e.outerRadius,e.thetaSegments,e.phiSegments,e.thetaStart,e.thetaLength)}};var Lt=class n extends Yt{constructor(e=1,t=32,i=16,r=0,s=Math.PI*2,o=0,a=Math.PI){super(),this.type="SphereGeometry",this.parameters={radius:e,widthSegments:t,heightSegments:i,phiStart:r,phiLength:s,thetaStart:o,thetaLength:a},t=Math.max(3,Math.floor(t)),i=Math.max(2,Math.floor(i));let l=Math.min(o+a,Math.PI),c=0,h=[],d=new G,u=new G,p=[],x=[],v=[],m=[];for(let f=0;f<=i;f++){let b=[],P=f/i,w=o+P*a,T=e*Math.cos(w),E=Math.sqrt(e*e-T*T),D=0;f===0&&o===0?D=.5/t:f===i&&l===Math.PI&&(D=-.5/t);for(let g=0;g<=t;g++){let S=g/t,_=r+S*s;d.x=-E*Math.cos(_),d.y=T,d.z=E*Math.sin(_),x.push(d.x,d.y,d.z),u.copy(d).normalize(),v.push(u.x,u.y,u.z),m.push(S+D,1-P),b.push(c++)}h.push(b)}for(let f=0;f<i;f++)for(let b=0;b<t;b++){let P=h[f][b+1],w=h[f][b],T=h[f+1][b],E=h[f+1][b+1];(f!==0||o>0)&&p.push(P,w,E),(f!==i-1||l<Math.PI)&&p.push(w,T,E)}this.setIndex(p),this.setAttribute("position",new _t(x,3)),this.setAttribute("normal",new _t(v,3)),this.setAttribute("uv",new _t(m,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radius,e.widthSegments,e.heightSegments,e.phiStart,e.phiLength,e.thetaStart,e.thetaLength)}};var Un=class n extends Yt{constructor(e=1,t=.4,i=12,r=48,s=Math.PI*2,o=0,a=Math.PI*2){super(),this.type="TorusGeometry",this.parameters={radius:e,tube:t,radialSegments:i,tubularSegments:r,arc:s,thetaStart:o,thetaLength:a},i=Math.floor(i),r=Math.floor(r);let l=[],c=[],h=[],d=[],u=new G,p=new G,x=new G;for(let v=0;v<=i;v++){let m=o+v/i*a;for(let f=0;f<=r;f++){let b=f/r*s;p.x=(e+t*Math.cos(m))*Math.cos(b),p.y=(e+t*Math.cos(m))*Math.sin(b),p.z=t*Math.sin(m),c.push(p.x,p.y,p.z),u.x=e*Math.cos(b),u.y=e*Math.sin(b),x.subVectors(p,u).normalize(),h.push(x.x,x.y,x.z),d.push(f/r),d.push(v/i)}}for(let v=1;v<=i;v++)for(let m=1;m<=r;m++){let f=(r+1)*v+m-1,b=(r+1)*(v-1)+m-1,P=(r+1)*(v-1)+m,w=(r+1)*v+m;l.push(f,b,w),l.push(b,P,w)}this.setIndex(l),this.setAttribute("position",new _t(c,3)),this.setAttribute("normal",new _t(h,3)),this.setAttribute("uv",new _t(d,2))}copy(e){return super.copy(e),this.parameters=Object.assign({},e.parameters),this}static fromJSON(e){return new n(e.radius,e.tube,e.radialSegments,e.tubularSegments,e.arc,e.thetaStart,e.thetaLength)}};var js=class extends yi{constructor(e){super(),this.isShadowMaterial=!0,this.type="ShadowMaterial",this.color=new ke(0),this.transparent=!0,this.fog=!0,this.setValues(e)}copy(e){return super.copy(e),this.color.copy(e.color),this.fog=e.fog,this}};function Er(n){let e={};for(let t in n){e[t]={};for(let i in n[t]){let r=n[t][i];if(_h(r))r.isRenderTargetTexture?(Ye("UniformsUtils: Textures of render targets cannot be cloned via cloneUniforms() or mergeUniforms()."),e[t][i]=null):e[t][i]=r.clone();else if(Array.isArray(r))if(_h(r[0])){let s=[];for(let o=0,a=r.length;o<a;o++)s[o]=r[o].clone();e[t][i]=s}else e[t][i]=r.slice();else e[t][i]=r}}return e}function Tn(n){let e={};for(let t=0;t<n.length;t++){let i=Er(n[t]);for(let r in i)e[r]=i[r]}return e}function _h(n){return n&&(n.isColor||n.isMatrix3||n.isMatrix4||n.isVector2||n.isVector3||n.isVector4||n.isTexture||n.isQuaternion)}function sm(n){let e=[];for(let t=0;t<n.length;t++)e.push(n[t].clone());return e}function Qc(n){let e=n.getRenderTarget();return e===null?n.outputColorSpace:e.isXRRenderTarget===!0?e.texture.colorSpace:ht.workingColorSpace}var bl={clone:Er,merge:Tn},om=`void main() {
	gl_Position = projectionMatrix * modelViewMatrix * vec4( position, 1.0 );
}`,am=`void main() {
	gl_FragColor = vec4( 1.0, 0.0, 0.0, 1.0 );
}`,En=class extends yi{constructor(e){super(),this.isShaderMaterial=!0,this.type="ShaderMaterial",this.defines={},this.uniforms={},this.uniformsGroups=[],this.vertexShader=om,this.fragmentShader=am,this.linewidth=1,this.wireframe=!1,this.wireframeLinewidth=1,this.fog=!1,this.lights=!1,this.clipping=!1,this.forceSinglePass=!0,this.extensions={clipCullDistance:!1,multiDraw:!1},this.defaultAttributeValues={color:[1,1,1],uv:[0,0],uv1:[0,0]},this.index0AttributeName=void 0,this.uniformsNeedUpdate=!1,this.glslVersion=null,e!==void 0&&this.setValues(e)}copy(e){return super.copy(e),this.fragmentShader=e.fragmentShader,this.vertexShader=e.vertexShader,this.uniforms=Er(e.uniforms),this.uniformsGroups=sm(e.uniformsGroups),this.defines=Object.assign({},e.defines),this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this.fog=e.fog,this.lights=e.lights,this.clipping=e.clipping,this.extensions=Object.assign({},e.extensions),this.glslVersion=e.glslVersion,this.defaultAttributeValues=Object.assign({},e.defaultAttributeValues),this.index0AttributeName=e.index0AttributeName,this.uniformsNeedUpdate=e.uniformsNeedUpdate,this}toJSON(e){let t=super.toJSON(e);t.glslVersion=this.glslVersion,t.uniforms={};for(let r in this.uniforms){let o=this.uniforms[r].value;o&&o.isTexture?t.uniforms[r]={type:"t",value:o.toJSON(e).uuid}:o&&o.isColor?t.uniforms[r]={type:"c",value:o.getHex()}:o&&o.isVector2?t.uniforms[r]={type:"v2",value:o.toArray()}:o&&o.isVector3?t.uniforms[r]={type:"v3",value:o.toArray()}:o&&o.isVector4?t.uniforms[r]={type:"v4",value:o.toArray()}:o&&o.isMatrix3?t.uniforms[r]={type:"m3",value:o.toArray()}:o&&o.isMatrix4?t.uniforms[r]={type:"m4",value:o.toArray()}:t.uniforms[r]={value:o}}Object.keys(this.defines).length>0&&(t.defines=this.defines),t.vertexShader=this.vertexShader,t.fragmentShader=this.fragmentShader,t.lights=this.lights,t.clipping=this.clipping;let i={};for(let r in this.extensions)this.extensions[r]===!0&&(i[r]=!0);return Object.keys(i).length>0&&(t.extensions=i),t}fromJSON(e,t){if(super.fromJSON(e,t),e.uniforms!==void 0)for(let i in e.uniforms){let r=e.uniforms[i];switch(this.uniforms[i]={},r.type){case"t":this.uniforms[i].value=t[r.value]||null;break;case"c":this.uniforms[i].value=new ke().setHex(r.value);break;case"v2":this.uniforms[i].value=new Xe().fromArray(r.value);break;case"v3":this.uniforms[i].value=new G().fromArray(r.value);break;case"v4":this.uniforms[i].value=new Vt().fromArray(r.value);break;case"m3":this.uniforms[i].value=new je().fromArray(r.value);break;case"m4":this.uniforms[i].value=new wt().fromArray(r.value);break;default:this.uniforms[i].value=r.value}}if(e.defines!==void 0&&(this.defines=e.defines),e.vertexShader!==void 0&&(this.vertexShader=e.vertexShader),e.fragmentShader!==void 0&&(this.fragmentShader=e.fragmentShader),e.glslVersion!==void 0&&(this.glslVersion=e.glslVersion),e.extensions!==void 0)for(let i in e.extensions)this.extensions[i]=e.extensions[i];return e.lights!==void 0&&(this.lights=e.lights),e.clipping!==void 0&&(this.clipping=e.clipping),this}},xa=class extends En{constructor(e){super(e),this.isRawShaderMaterial=!0,this.type="RawShaderMaterial"}};var Qs=class extends yi{constructor(e){super(),this.isMeshToonMaterial=!0,this.defines={TOON:""},this.type="MeshToonMaterial",this.color=new ke(16777215),this.map=null,this.gradientMap=null,this.lightMap=null,this.lightMapIntensity=1,this.aoMap=null,this.aoMapIntensity=1,this.emissive=new ke(0),this.emissiveIntensity=1,this.emissiveMap=null,this.bumpMap=null,this.bumpScale=1,this.normalMap=null,this.normalMapType=vl,this.normalScale=new Xe(1,1),this.displacementMap=null,this.displacementScale=1,this.displacementBias=0,this.alphaMap=null,this.wireframe=!1,this.wireframeLinewidth=1,this.wireframeLinecap="round",this.wireframeLinejoin="round",this.fog=!0,this.setValues(e)}copy(e){return super.copy(e),this.color.copy(e.color),this.map=e.map,this.gradientMap=e.gradientMap,this.lightMap=e.lightMap,this.lightMapIntensity=e.lightMapIntensity,this.aoMap=e.aoMap,this.aoMapIntensity=e.aoMapIntensity,this.emissive.copy(e.emissive),this.emissiveMap=e.emissiveMap,this.emissiveIntensity=e.emissiveIntensity,this.bumpMap=e.bumpMap,this.bumpScale=e.bumpScale,this.normalMap=e.normalMap,this.normalMapType=e.normalMapType,this.normalScale.copy(e.normalScale),this.displacementMap=e.displacementMap,this.displacementScale=e.displacementScale,this.displacementBias=e.displacementBias,this.alphaMap=e.alphaMap,this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this.wireframeLinecap=e.wireframeLinecap,this.wireframeLinejoin=e.wireframeLinejoin,this.fog=e.fog,this}};var va=class extends yi{constructor(e){super(),this.isMeshDepthMaterial=!0,this.type="MeshDepthMaterial",this.depthPacking=jh,this.map=null,this.alphaMap=null,this.displacementMap=null,this.displacementScale=1,this.displacementBias=0,this.wireframe=!1,this.wireframeLinewidth=1,this.setValues(e)}copy(e){return super.copy(e),this.depthPacking=e.depthPacking,this.map=e.map,this.alphaMap=e.alphaMap,this.displacementMap=e.displacementMap,this.displacementScale=e.displacementScale,this.displacementBias=e.displacementBias,this.wireframe=e.wireframe,this.wireframeLinewidth=e.wireframeLinewidth,this}},ya=class extends yi{constructor(e){super(),this.isMeshDistanceMaterial=!0,this.type="MeshDistanceMaterial",this.map=null,this.alphaMap=null,this.displacementMap=null,this.displacementScale=1,this.displacementBias=0,this.setValues(e)}copy(e){return super.copy(e),this.map=e.map,this.alphaMap=e.alphaMap,this.displacementMap=e.displacementMap,this.displacementScale=e.displacementScale,this.displacementBias=e.displacementBias,this}};function Yr(n,e){return!n||n.constructor===e?n:typeof e.BYTES_PER_ELEMENT=="number"?new e(n):Array.prototype.slice.call(n)}function xc(n){return n!==void 0&&n.inTangents!==void 0&&n.outTangents!==void 0}var Ki=class{constructor(e,t,i,r){this.parameterPositions=e,this._cachedIndex=0,this.resultBuffer=r!==void 0?r:new t.constructor(i),this.sampleValues=t,this.valueSize=i,this.settings=null,this.DefaultSettings_={}}evaluate(e){let t=this.parameterPositions,i=this._cachedIndex,r=t[i],s=t[i-1];n:{e:{let o;t:{i:if(!(e<r)){for(let a=i+2;;){if(r===void 0){if(e<s)break i;return i=t.length,this._cachedIndex=i,this.copySampleValue_(i-1)}if(i===a)break;if(s=r,r=t[++i],e<r)break e}o=t.length;break t}if(!(e>=s)){let a=t[1];e<a&&(i=2,s=a);for(let l=i-2;;){if(s===void 0)return this._cachedIndex=0,this.copySampleValue_(0);if(i===l)break;if(r=s,s=t[--i-1],e>=s)break e}o=i,i=0;break t}break n}for(;i<o;){let a=i+o>>>1;e<t[a]?o=a:i=a+1}if(r=t[i],s=t[i-1],s===void 0)return this._cachedIndex=0,this.copySampleValue_(0);if(r===void 0)return i=t.length,this._cachedIndex=i,this.copySampleValue_(i-1)}this._cachedIndex=i,this.intervalChanged_(i,s,r)}return this.interpolate_(i,s,e,r)}getSettings_(){return this.settings||this.DefaultSettings_}copySampleValue_(e){let t=this.resultBuffer,i=this.sampleValues,r=this.valueSize,s=e*r;for(let o=0;o!==r;++o)t[o]=i[s+o];return t}interpolate_(){throw new Error("THREE.Interpolant: Call to abstract method.")}intervalChanged_(){}},_a=class extends Ki{constructor(e,t,i,r){super(e,t,i,r),this._weightPrev=-0,this._offsetPrev=-0,this._weightNext=-0,this._offsetNext=-0,this.DefaultSettings_={endingStart:_c,endingEnd:_c}}intervalChanged_(e,t,i){let r=this.parameterPositions,s=e-2,o=e+1,a=r[s],l=r[o];if(a===void 0)switch(this.getSettings_().endingStart){case bc:s=e,a=2*t-i;break;case Mc:s=r.length-2,a=t+r[s]-r[s+1];break;default:s=e,a=i}if(l===void 0)switch(this.getSettings_().endingEnd){case bc:o=e,l=2*i-t;break;case Mc:o=1,l=i+r[1]-r[0];break;default:o=e-1,l=t}let c=(i-t)*.5,h=this.valueSize;this._weightPrev=c/(t-a),this._weightNext=c/(l-i),this._offsetPrev=s*h,this._offsetNext=o*h}interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,l=e*a,c=l-a,h=this._offsetPrev,d=this._offsetNext,u=this._weightPrev,p=this._weightNext,x=(i-t)/(r-t),v=x*x,m=v*x,f=-u*m+2*u*v-u*x,b=(1+u)*m+(-1.5-2*u)*v+(-.5+u)*x+1,P=(-1-p)*m+(1.5+p)*v+.5*x,w=p*m-p*v;for(let T=0;T!==a;++T)s[T]=f*o[h+T]+b*o[c+T]+P*o[l+T]+w*o[d+T];return s}},ba=class extends Ki{constructor(e,t,i,r){super(e,t,i,r)}interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,l=e*a,c=l-a,h=(i-t)/(r-t),d=1-h;for(let u=0;u!==a;++u)s[u]=o[c+u]*d+o[l+u]*h;return s}},Ma=class extends Ki{constructor(e,t,i,r){super(e,t,i,r)}interpolate_(e){return this.copySampleValue_(e-1)}},Sa=class extends Ki{interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,l=e*a,c=l-a,h=this.inTangents,d=this.outTangents;if(!h||!d){let x=(i-t)/(r-t),v=1-x;for(let m=0;m!==a;++m)s[m]=o[c+m]*v+o[l+m]*x;return s}let u=a*2,p=e-1;for(let x=0;x!==a;++x){let v=o[c+x],m=o[l+x],f=p*u+x*2,b=d[f],P=d[f+1],w=e*u+x*2,T=h[w],E=h[w+1],D=cm(i,t,b,T,r);s[x]=fd(D,v,P,E,m)}return s}};function fd(n,e,t,i,r){let s=1-n;return s*s*s*e+3*s*s*n*t+3*s*n*n*i+n*n*n*r}function lm(n,e,t,i,r){let s=1-n;return 3*s*s*(t-e)+6*s*n*(i-t)+3*n*n*(r-i)}function cm(n,e,t,i,r){let s=(n-e)/(r-e);for(let o=0;o<8;o++){let a=fd(s,e,t,i,r)-n;if(Math.abs(a)<1e-10)break;let l=lm(s,e,t,i,r);if(Math.abs(l)<1e-10)break;s=Math.max(0,Math.min(1,s-a/l))}return s}var qn=class{constructor(e,t,i,r){if(e===void 0)throw new Error("THREE.KeyframeTrack: track name is undefined");if(t===void 0||t.length===0)throw new Error("THREE.KeyframeTrack: no keyframes in track named "+e);this.name=e,this.times=Yr(t,this.TimeBufferType),this.values=Yr(i,this.ValueBufferType),this.setInterpolation(r||this.DefaultInterpolation)}static toJSON(e){let t=e.constructor,i;if(t.toJSON!==this.toJSON)i=t.toJSON(e);else{i={name:e.name,times:Yr(e.times,Array),values:Yr(e.values,Array)};let r=e.getInterpolation();r!==e.DefaultInterpolation&&(i.interpolation=r),xc(e.settings)&&(i.settings={inTangents:Yr(e.settings.inTangents,Array),outTangents:Yr(e.settings.outTangents,Array)})}return i.type=e.ValueTypeName,i}InterpolantFactoryMethodDiscrete(e){return new Ma(this.times,this.values,this.getValueSize(),e)}InterpolantFactoryMethodLinear(e){return new ba(this.times,this.values,this.getValueSize(),e)}InterpolantFactoryMethodSmooth(e){return new _a(this.times,this.values,this.getValueSize(),e)}InterpolantFactoryMethodBezier(e){let t=new Sa(this.times,this.values,this.getValueSize(),e);return this.settings&&(t.inTangents=this.settings.inTangents,t.outTangents=this.settings.outTangents),t}setInterpolation(e){let t;switch(e){case Fs:t=this.InterpolantFactoryMethodDiscrete;break;case ha:t=this.InterpolantFactoryMethodLinear;break;case ta:t=this.InterpolantFactoryMethodSmooth;break;case yc:t=this.InterpolantFactoryMethodBezier;break}if(t===void 0){let i="unsupported interpolation for "+this.ValueTypeName+" keyframe track named "+this.name;if(this.createInterpolant===void 0)if(e!==this.DefaultInterpolation)this.setInterpolation(this.DefaultInterpolation);else throw new Error(i);return Ye("KeyframeTrack:",i),this}return this.createInterpolant=t,this}getInterpolation(){switch(this.createInterpolant){case this.InterpolantFactoryMethodDiscrete:return Fs;case this.InterpolantFactoryMethodLinear:return ha;case this.InterpolantFactoryMethodSmooth:return ta;case this.InterpolantFactoryMethodBezier:return yc}}getValueSize(){return this.values.length/this.times.length}shift(e){if(e!==0){let t=this.times;for(let i=0,r=t.length;i!==r;++i)t[i]+=e}return this}scale(e){if(e!==1){let t=this.times;for(let i=0,r=t.length;i!==r;++i)t[i]*=e;xc(this.settings)&&(bh(this.settings.inTangents,e),bh(this.settings.outTangents,e))}return this}trim(e,t){let i=this.times,r=i.length,s=0,o=r-1;for(;s!==r&&i[s]<e;)++s;for(;o!==-1&&i[o]>t;)--o;if(++o,s!==0||o!==r){s>=o&&(o=Math.max(o,1),s=o-1);let a=this.getValueSize();this.times=i.slice(s,o),this.values=this.values.slice(s*a,o*a)}return this}validate(){let e=!0,t=this.getValueSize();t-Math.floor(t)!==0&&(Ke("KeyframeTrack: Invalid value size in track.",this),e=!1);let i=this.times,r=this.values,s=i.length;s===0&&(Ke("KeyframeTrack: Track is empty.",this),e=!1);let o=null;for(let a=0;a!==s;a++){let l=i[a];if(typeof l=="number"&&isNaN(l)){Ke("KeyframeTrack: Time is not a valid number.",this,a,l),e=!1;break}if(o!==null&&o>l){Ke("KeyframeTrack: Out of order keys.",this,a,l,o),e=!1;break}o=l}if(r!==void 0&&Sp(r))for(let a=0,l=r.length;a!==l;++a){let c=r[a];if(isNaN(c)){Ke("KeyframeTrack: Value is not a valid number.",this,a,c),e=!1;break}}return e}optimize(){let e=this.times.slice(),t=this.values.slice(),i=this.getValueSize(),r=this.getInterpolation()===ta,s=e.length-1,o=1;for(let a=1;a<s;++a){let l=!1,c=e[a],h=e[a+1];if(c!==h&&(a!==1||c!==e[0]))if(r)l=!0;else{let d=a*i,u=d-i,p=d+i;for(let x=0;x!==i;++x){let v=t[d+x];if(v!==t[u+x]||v!==t[p+x]){l=!0;break}}}if(l){if(a!==o){e[o]=e[a];let d=a*i,u=o*i;for(let p=0;p!==i;++p)t[u+p]=t[d+p]}++o}}if(s>0){e[o]=e[s];for(let a=s*i,l=o*i,c=0;c!==i;++c)t[l+c]=t[a+c];++o}return o!==e.length?(this.times=e.slice(0,o),this.values=t.slice(0,o*i)):(this.times=e,this.values=t),this}clone(){let e=this.times.slice(),t=this.values.slice(),i=this.constructor,r=new i(this.name,e,t);return r.createInterpolant=this.createInterpolant,xc(this.settings)&&(r.settings={inTangents:this.settings.inTangents.slice(),outTangents:this.settings.outTangents.slice()}),r}};function bh(n,e){for(let t=0,i=n.length;t!==i;t+=2)n[t]*=e}qn.prototype.ValueTypeName="";qn.prototype.TimeBufferType=Float32Array;qn.prototype.ValueBufferType=Float32Array;qn.prototype.DefaultInterpolation=ha;var Ji=class extends qn{constructor(e,t,i){super(e,t,i)}};Ji.prototype.ValueTypeName="bool";Ji.prototype.ValueBufferType=Array;Ji.prototype.DefaultInterpolation=Fs;Ji.prototype.InterpolantFactoryMethodLinear=void 0;Ji.prototype.InterpolantFactoryMethodSmooth=void 0;var wa=class extends qn{constructor(e,t,i,r){super(e,t,i,r)}};wa.prototype.ValueTypeName="color";var Ea=class extends qn{constructor(e,t,i,r){super(e,t,i,r)}};Ea.prototype.ValueTypeName="number";var Ta=class extends Ki{constructor(e,t,i,r){super(e,t,i,r)}interpolate_(e,t,i,r){let s=this.resultBuffer,o=this.sampleValues,a=this.valueSize,l=(i-t)/(r-t),c=e*a;for(let h=c+a;c!==h;c+=4)un.slerpFlat(s,0,o,c-a,o,c,l);return s}},eo=class extends qn{constructor(e,t,i,r){super(e,t,i,r)}InterpolantFactoryMethodLinear(e){return new Ta(this.times,this.values,this.getValueSize(),e)}};eo.prototype.ValueTypeName="quaternion";eo.prototype.InterpolantFactoryMethodSmooth=void 0;var ji=class extends qn{constructor(e,t,i){super(e,t,i)}};ji.prototype.ValueTypeName="string";ji.prototype.ValueBufferType=Array;ji.prototype.DefaultInterpolation=Fs;ji.prototype.InterpolantFactoryMethodLinear=void 0;ji.prototype.InterpolantFactoryMethodSmooth=void 0;var Aa=class extends qn{constructor(e,t,i,r){super(e,t,i,r)}};Aa.prototype.ValueTypeName="vector";var Ra=class{constructor(e,t,i){let r=this,s=!1,o=0,a=0,l,c=[];this.onStart=void 0,this.onLoad=e,this.onProgress=t,this.onError=i,this._abortController=null,this.itemStart=function(h){a++,s===!1&&r.onStart!==void 0&&r.onStart(h,o,a),s=!0},this.itemEnd=function(h){o++,r.onProgress!==void 0&&r.onProgress(h,o,a),o===a&&(s=!1,r.onLoad!==void 0&&r.onLoad())},this.itemError=function(h){r.onError!==void 0&&r.onError(h)},this.resolveURL=function(h){return h=h.normalize("NFC"),l?l(h):h},this.setURLModifier=function(h){return l=h,this},this.addHandler=function(h,d){return c.push(h,d),this},this.removeHandler=function(h){let d=c.indexOf(h);return d!==-1&&c.splice(d,2),this},this.getHandler=function(h){for(let d=0,u=c.length;d<u;d+=2){let p=c[d],x=c[d+1];if(p.global&&(p.lastIndex=0),p.test(h))return x}return null},this.abort=function(){return this.abortController.abort(),this._abortController=null,this}}get abortController(){return this._abortController||(this._abortController=new AbortController),this._abortController}},pd=new Ra,Ca=class{constructor(e){this.manager=e!==void 0?e:pd,this.crossOrigin="anonymous",this.withCredentials=!1,this.path="",this.resourcePath="",this.requestHeader={},typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("observe",{detail:this}))}load(){}loadAsync(e,t){let i=this;return new Promise(function(r,s){i.load(e,r,t,s)})}parse(){}setCrossOrigin(e){return this.crossOrigin=e,this}setWithCredentials(e){return this.withCredentials=e,this}setPath(e){return this.path=e,this}setResourcePath(e){return this.resourcePath=e,this}setRequestHeader(e){return this.requestHeader=e,this}abort(){return this}};Ca.DEFAULT_MATERIAL_NAME="__DEFAULT";var ls=class extends yn{constructor(e,t=1){super(),this.isLight=!0,this.type="Light",this.color=new ke(e),this.intensity=t}copy(e,t){return super.copy(e,t),this.color.copy(e.color),this.intensity=e.intensity,this}toJSON(e){let t=super.toJSON(e);return t.object.color=this.color.getHex(),t.object.intensity=this.intensity,t}},_r=class extends ls{constructor(e,t,i){super(e,i),this.isHemisphereLight=!0,this.type="HemisphereLight",this.position.copy(yn.DEFAULT_UP),this.updateMatrix(),this.groundColor=new ke(t)}copy(e,t){return super.copy(e,t),this.groundColor.copy(e.groundColor),this}toJSON(e){let t=super.toJSON(e);return t.object.groundColor=this.groundColor.getHex(),t}},vc=new wt,Mh=new G,Sh=new G,to=class{constructor(e){this.camera=e,this.intensity=1,this.bias=0,this.biasNode=null,this.normalBias=0,this.radius=1,this.blurSamples=8,this.mapSize=new Xe(512,512),this.mapType=On,this.map=null,this.mapPass=null,this.matrix=new wt,this.autoUpdate=!0,this.needsUpdate=!1,this._frustum=new os,this._frameExtents=new Xe(1,1),this._viewportCount=1,this._viewports=[new Vt(0,0,1,1)]}getViewportCount(){return this._viewportCount}getCamera(){return this.camera}getFrustum(){return this._frustum}updateMatrices(e){let t=this.camera;Mh.setFromMatrixPosition(e.matrixWorld),t.position.copy(Mh),Sh.setFromMatrixPosition(e.target.matrixWorld),t.lookAt(Sh),t.updateMatrixWorld(),this._updateMatrix(t,this.matrix,this._frustum)}_updateMatrix(e,t,i,r){vc.multiplyMatrices(e.projectionMatrix,e.matrixWorldInverse),i.setFromProjectionMatrix(vc,e.coordinateSystem,e.reversedDepth);let s=this._frameExtents,o=r?r.z/s.x:1,a=r?r.w/s.y:1,l=r?r.x/s.x:0,c=r?r.y/s.y:0;e.coordinateSystem===es||e.reversedDepth?t.set(.5*o,0,0,.5*o+l,0,.5*a,0,.5*a+c,0,0,1,0,0,0,0,1):t.set(.5*o,0,0,.5*o+l,0,.5*a,0,.5*a+c,0,0,.5,.5,0,0,0,1),t.multiply(vc)}getViewport(e){return this._viewports[e]}getFrameExtents(){return this._frameExtents}dispose(){this.map&&this.map.dispose(),this.mapPass&&this.mapPass.dispose()}copy(e){return this.camera=e.camera.clone(),this.intensity=e.intensity,this.bias=e.bias,this.radius=e.radius,this.autoUpdate=e.autoUpdate,this.needsUpdate=e.needsUpdate,this.normalBias=e.normalBias,this.blurSamples=e.blurSamples,this.mapSize.copy(e.mapSize),this.biasNode=e.biasNode,this}clone(){return new this.constructor().copy(this)}toJSON(){let e={};return e.intensity=this.intensity,e.bias=this.bias,e.normalBias=this.normalBias,e.radius=this.radius,e.blurSamples=this.blurSamples,e.mapSize=this.mapSize.toArray(),e.camera=this.camera.toJSON(!1).object,delete e.camera.matrix,e}},Qo=new G,ea=new un,gi=new G,no=class extends yn{constructor(){super(),this.isCamera=!0,this.type="Camera",this.matrixWorldInverse=new wt,this.projectionMatrix=new wt,this.projectionMatrixInverse=new wt,this.coordinateSystem=ai,this._reversedDepth=!1}get reversedDepth(){return this._reversedDepth}copy(e,t){return super.copy(e,t),this.matrixWorldInverse.copy(e.matrixWorldInverse),this.projectionMatrix.copy(e.projectionMatrix),this.projectionMatrixInverse.copy(e.projectionMatrixInverse),this.coordinateSystem=e.coordinateSystem,this}getWorldDirection(e){return super.getWorldDirection(e).negate()}updateMatrixWorld(e){super.updateMatrixWorld(e),this.matrixWorld.decompose(Qo,ea,gi),gi.x===1&&gi.y===1&&gi.z===1?this.matrixWorldInverse.copy(this.matrixWorld).invert():this.matrixWorldInverse.compose(Qo,ea,gi.set(1,1,1)).invert()}updateWorldMatrix(e,t,i=!1){super.updateWorldMatrix(e,t,i),this.matrixWorld.decompose(Qo,ea,gi),gi.x===1&&gi.y===1&&gi.z===1?this.matrixWorldInverse.copy(this.matrixWorld).invert():this.matrixWorldInverse.compose(Qo,ea,gi.set(1,1,1)).invert()}clone(){return new this.constructor().copy(this)}},Xi=new G,wh=new Xe,Eh=new Xe,cn=class extends no{constructor(e=50,t=1,i=.1,r=2e3){super(),this.isPerspectiveCamera=!0,this.type="PerspectiveCamera",this.fov=e,this.zoom=1,this.near=i,this.far=r,this.focus=10,this.aspect=t,this.view=null,this.filmGauge=35,this.filmOffset=0,this.updateProjectionMatrix()}copy(e,t){return super.copy(e,t),this.fov=e.fov,this.zoom=e.zoom,this.near=e.near,this.far=e.far,this.focus=e.focus,this.aspect=e.aspect,this.view=e.view===null?null:Object.assign({},e.view),this.filmGauge=e.filmGauge,this.filmOffset=e.filmOffset,this}setFocalLength(e){let t=.5*this.getFilmHeight()/e;this.fov=ns*2*Math.atan(t),this.updateProjectionMatrix()}getFocalLength(){let e=Math.tan(Ns*.5*this.fov);return .5*this.getFilmHeight()/e}getEffectiveFOV(){return ns*2*Math.atan(Math.tan(Ns*.5*this.fov)/this.zoom)}getFilmWidth(){return this.filmGauge*Math.min(this.aspect,1)}getFilmHeight(){return this.filmGauge/Math.max(this.aspect,1)}getViewBounds(e,t,i){Xi.set(-1,-1,.5).applyMatrix4(this.projectionMatrixInverse),t.set(Xi.x,Xi.y).multiplyScalar(-e/Xi.z),Xi.set(1,1,.5).applyMatrix4(this.projectionMatrixInverse),i.set(Xi.x,Xi.y).multiplyScalar(-e/Xi.z)}getViewSize(e,t){return this.getViewBounds(e,wh,Eh),t.subVectors(Eh,wh)}setViewOffset(e,t,i,r,s,o){this.aspect=e/t,this.view===null&&(this.view={enabled:!0,fullWidth:1,fullHeight:1,offsetX:0,offsetY:0,width:1,height:1}),this.view.enabled=!0,this.view.fullWidth=e,this.view.fullHeight=t,this.view.offsetX=i,this.view.offsetY=r,this.view.width=s,this.view.height=o,this.updateProjectionMatrix()}clearViewOffset(){this.view!==null&&(this.view.enabled=!1),this.updateProjectionMatrix()}updateProjectionMatrix(){let e=this.near,t=e*Math.tan(Ns*.5*this.fov)/this.zoom,i=2*t,r=this.aspect*i,s=-.5*r,o=this.view;if(this.view!==null&&this.view.enabled){let l=o.fullWidth,c=o.fullHeight;s+=o.offsetX*r/l,t-=o.offsetY*i/c,r*=o.width/l,i*=o.height/c}let a=this.filmOffset;a!==0&&(s+=e*a/this.getFilmWidth()),this.projectionMatrix.makePerspective(s,s+r,t,t-i,e,this.far,this.coordinateSystem,this.reversedDepth),this.projectionMatrixInverse.copy(this.projectionMatrix).invert()}toJSON(e){let t=super.toJSON(e);return t.object.fov=this.fov,t.object.zoom=this.zoom,t.object.near=this.near,t.object.far=this.far,t.object.focus=this.focus,t.object.aspect=this.aspect,this.view!==null&&(t.object.view=Object.assign({},this.view)),t.object.filmGauge=this.filmGauge,t.object.filmOffset=this.filmOffset,t}};var wc=class extends to{constructor(){super(new cn(90,1,.5,500)),this.isPointLightShadow=!0}},ki=class extends ls{constructor(e,t,i=0,r=2){super(e,t),this.isPointLight=!0,this.type="PointLight",this.distance=i,this.decay=r,this.shadow=new wc}get power(){return this.intensity*4*Math.PI}set power(e){this.intensity=e/(4*Math.PI)}dispose(){super.dispose(),this.shadow.dispose()}copy(e,t){return super.copy(e,t),this.distance=e.distance,this.decay=e.decay,this.shadow=e.shadow.clone(),this}toJSON(e){let t=super.toJSON(e);return t.object.distance=this.distance,t.object.decay=this.decay,t.object.shadow=this.shadow.toJSON(),t}},cs=class extends no{constructor(e=-1,t=1,i=1,r=-1,s=.1,o=2e3){super(),this.isOrthographicCamera=!0,this.type="OrthographicCamera",this.zoom=1,this.view=null,this.left=e,this.right=t,this.top=i,this.bottom=r,this.near=s,this.far=o,this.updateProjectionMatrix()}copy(e,t){return super.copy(e,t),this.left=e.left,this.right=e.right,this.top=e.top,this.bottom=e.bottom,this.near=e.near,this.far=e.far,this.zoom=e.zoom,this.view=e.view===null?null:Object.assign({},e.view),this}setViewOffset(e,t,i,r,s,o){this.view===null&&(this.view={enabled:!0,fullWidth:1,fullHeight:1,offsetX:0,offsetY:0,width:1,height:1}),this.view.enabled=!0,this.view.fullWidth=e,this.view.fullHeight=t,this.view.offsetX=i,this.view.offsetY=r,this.view.width=s,this.view.height=o,this.updateProjectionMatrix()}clearViewOffset(){this.view!==null&&(this.view.enabled=!1),this.updateProjectionMatrix()}updateProjectionMatrix(){let e=(this.right-this.left)/(2*this.zoom),t=(this.top-this.bottom)/(2*this.zoom),i=(this.right+this.left)/2,r=(this.top+this.bottom)/2,s=i-e,o=i+e,a=r+t,l=r-t;if(this.view!==null&&this.view.enabled){let c=(this.right-this.left)/this.view.fullWidth/this.zoom,h=(this.top-this.bottom)/this.view.fullHeight/this.zoom;s+=c*this.view.offsetX,o=s+c*this.view.width,a-=h*this.view.offsetY,l=a-h*this.view.height}this.projectionMatrix.makeOrthographic(s,o,a,l,this.near,this.far,this.coordinateSystem,this.reversedDepth),this.projectionMatrixInverse.copy(this.projectionMatrix).invert()}toJSON(e){let t=super.toJSON(e);return t.object.zoom=this.zoom,t.object.left=this.left,t.object.right=this.right,t.object.top=this.top,t.object.bottom=this.bottom,t.object.near=this.near,t.object.far=this.far,this.view!==null&&(t.object.view=Object.assign({},this.view)),t}},Ec=class extends to{constructor(){super(new cs(-5,5,5,-5,.5,500)),this.isDirectionalLightShadow=!0}},br=class extends ls{constructor(e,t){super(e,t),this.isDirectionalLight=!0,this.type="DirectionalLight",this.position.copy(yn.DEFAULT_UP),this.updateMatrix(),this.target=new yn,this.shadow=new Ec}dispose(){super.dispose(),this.shadow.dispose()}copy(e){return super.copy(e),this.target=e.target.clone(),this.shadow=e.shadow.clone(),this}toJSON(e){let t=super.toJSON(e);return t.object.shadow=this.shadow.toJSON(),t.object.target=this.target.uuid,t}};var Zr=-90,Kr=1,Pa=class extends yn{constructor(e,t,i){super(),this.type="CubeCamera",this.renderTarget=i,this.coordinateSystem=null,this.activeMipmapLevel=0;let r=new cn(Zr,Kr,e,t);r.layers=this.layers,this.add(r);let s=new cn(Zr,Kr,e,t);s.layers=this.layers,this.add(s);let o=new cn(Zr,Kr,e,t);o.layers=this.layers,this.add(o);let a=new cn(Zr,Kr,e,t);a.layers=this.layers,this.add(a);let l=new cn(Zr,Kr,e,t);l.layers=this.layers,this.add(l);let c=new cn(Zr,Kr,e,t);c.layers=this.layers,this.add(c)}updateCoordinateSystem(){let e=this.coordinateSystem,t=this.children.concat(),[i,r,s,o,a,l]=t;for(let c of t)this.remove(c);if(e===ai)i.up.set(0,1,0),i.lookAt(1,0,0),r.up.set(0,1,0),r.lookAt(-1,0,0),s.up.set(0,0,-1),s.lookAt(0,1,0),o.up.set(0,0,1),o.lookAt(0,-1,0),a.up.set(0,1,0),a.lookAt(0,0,1),l.up.set(0,1,0),l.lookAt(0,0,-1);else if(e===es)i.up.set(0,-1,0),i.lookAt(-1,0,0),r.up.set(0,-1,0),r.lookAt(1,0,0),s.up.set(0,0,1),s.lookAt(0,1,0),o.up.set(0,0,-1),o.lookAt(0,-1,0),a.up.set(0,-1,0),a.lookAt(0,0,1),l.up.set(0,-1,0),l.lookAt(0,0,-1);else throw new Error("THREE.CubeCamera.updateCoordinateSystem(): Invalid coordinate system: "+e);for(let c of t)this.add(c),c.updateMatrixWorld()}update(e,t){this.parent===null&&this.updateMatrixWorld();let{renderTarget:i,activeMipmapLevel:r}=this;this.coordinateSystem!==e.coordinateSystem&&(this.coordinateSystem=e.coordinateSystem,this.updateCoordinateSystem());let[s,o,a,l,c,h]=this.children,d=e.getRenderTarget(),u=e.getActiveCubeFace(),p=e.getActiveMipmapLevel(),x=e.xr.enabled;e.xr.enabled=!1;let v=i.texture.generateMipmaps;i.texture.generateMipmaps=!1;let m=!1;e.isWebGLRenderer===!0?m=e.state.buffers.depth.getReversed():m=e.reversedDepthBuffer,e.setRenderTarget(i,0,r),m&&e.autoClear===!1&&e.clearDepth(),e.render(t,s),e.setRenderTarget(i,1,r),m&&e.autoClear===!1&&e.clearDepth(),e.render(t,o),e.setRenderTarget(i,2,r),m&&e.autoClear===!1&&e.clearDepth(),e.render(t,a),e.setRenderTarget(i,3,r),m&&e.autoClear===!1&&e.clearDepth(),e.render(t,l),e.setRenderTarget(i,4,r),m&&e.autoClear===!1&&e.clearDepth(),e.render(t,c),i.texture.generateMipmaps=v,e.setRenderTarget(i,5,r),m&&e.autoClear===!1&&e.clearDepth(),e.render(t,h),e.setRenderTarget(d,u,p),e.xr.enabled=x,i.texture.needsPMREMUpdate=!0}},Ia=class extends cn{constructor(e=[]){super(),this.isArrayCamera=!0,this.isMultiViewCamera=!1,this.cameras=e}};var eu="\\[\\]\\.:\\/",um=new RegExp("["+eu+"]","g"),tu="[^"+eu+"]",hm="[^"+eu.replace("\\.","")+"]",dm=/((?:WC+[\/:])*)/.source.replace("WC",tu),fm=/(WCOD+)?/.source.replace("WCOD",hm),pm=/(?:\.(WC+)(?:\[(.+)\])?)?/.source.replace("WC",tu),mm=/\.(WC+)(?:\[(.+)\])?/.source.replace("WC",tu),gm=new RegExp("^"+dm+fm+pm+mm+"$"),xm=["material","materials","bones","map"],Tc=class{constructor(e,t,i){let r=i||Ft.parseTrackName(t);this._targetGroup=e,this._bindings=e.subscribe_(t,r)}getValue(e,t){this.bind();let i=this._targetGroup.nCachedObjects_,r=this._bindings[i];r!==void 0&&r.getValue(e,t)}setValue(e,t){let i=this._bindings;for(let r=this._targetGroup.nCachedObjects_,s=i.length;r!==s;++r)i[r].setValue(e,t)}bind(){let e=this._bindings;for(let t=this._targetGroup.nCachedObjects_,i=e.length;t!==i;++t)e[t].bind()}unbind(){let e=this._bindings;for(let t=this._targetGroup.nCachedObjects_,i=e.length;t!==i;++t)e[t].unbind()}},Ft=class n{constructor(e,t,i){this.path=t,this.parsedPath=i||n.parseTrackName(t),this.node=n.findNode(e,this.parsedPath.nodeName),this.rootNode=e,this.getValue=this._getValue_unbound,this.setValue=this._setValue_unbound}static create(e,t,i){return e&&e.isAnimationObjectGroup?new n.Composite(e,t,i):new n(e,t,i)}static sanitizeNodeName(e){return e.replace(/\s/g,"_").replace(um,"")}static parseTrackName(e){let t=gm.exec(e);if(t===null)throw new Error("THREE.PropertyBinding: Cannot parse trackName: "+e);let i={nodeName:t[2],objectName:t[3],objectIndex:t[4],propertyName:t[5],propertyIndex:t[6]},r=i.nodeName&&i.nodeName.lastIndexOf(".");if(r!==void 0&&r!==-1){let s=i.nodeName.substring(r+1);xm.indexOf(s)!==-1&&(i.nodeName=i.nodeName.substring(0,r),i.objectName=s)}if(i.propertyName===null||i.propertyName.length===0)throw new Error("THREE.PropertyBinding: can not parse propertyName from trackName: "+e);return i}static findNode(e,t){if(t===void 0||t===""||t==="."||t===-1||t===e.name||t===e.uuid)return e;if(e.skeleton){let i=e.skeleton.getBoneByName(t);if(i!==void 0)return i}if(e.children){let i=function(s){for(let o=0;o<s.length;o++){let a=s[o];if(a.name===t||a.uuid===t)return a;let l=i(a.children);if(l)return l}return null},r=i(e.children);if(r)return r}return null}_getValue_unavailable(){}_setValue_unavailable(){}_getValue_direct(e,t){e[t]=this.targetObject[this.propertyName]}_getValue_array(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)e[t++]=i[r]}_getValue_arrayElement(e,t){e[t]=this.resolvedProperty[this.propertyIndex]}_getValue_toArray(e,t){this.resolvedProperty.toArray(e,t)}_setValue_direct(e,t){this.targetObject[this.propertyName]=e[t]}_setValue_direct_setNeedsUpdate(e,t){this.targetObject[this.propertyName]=e[t],this.targetObject.needsUpdate=!0}_setValue_direct_setMatrixWorldNeedsUpdate(e,t){this.targetObject[this.propertyName]=e[t],this.targetObject.matrixWorldNeedsUpdate=!0}_setValue_array(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)i[r]=e[t++]}_setValue_array_setNeedsUpdate(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)i[r]=e[t++];this.targetObject.needsUpdate=!0}_setValue_array_setMatrixWorldNeedsUpdate(e,t){let i=this.resolvedProperty;for(let r=0,s=i.length;r!==s;++r)i[r]=e[t++];this.targetObject.matrixWorldNeedsUpdate=!0}_setValue_arrayElement(e,t){this.resolvedProperty[this.propertyIndex]=e[t]}_setValue_arrayElement_setNeedsUpdate(e,t){this.resolvedProperty[this.propertyIndex]=e[t],this.targetObject.needsUpdate=!0}_setValue_arrayElement_setMatrixWorldNeedsUpdate(e,t){this.resolvedProperty[this.propertyIndex]=e[t],this.targetObject.matrixWorldNeedsUpdate=!0}_setValue_fromArray(e,t){this.resolvedProperty.fromArray(e,t)}_setValue_fromArray_setNeedsUpdate(e,t){this.resolvedProperty.fromArray(e,t),this.targetObject.needsUpdate=!0}_setValue_fromArray_setMatrixWorldNeedsUpdate(e,t){this.resolvedProperty.fromArray(e,t),this.targetObject.matrixWorldNeedsUpdate=!0}_getValue_unbound(e,t){this.bind(),this.getValue(e,t)}_setValue_unbound(e,t){this.bind(),this.setValue(e,t)}bind(){let e=this.node,t=this.parsedPath,i=t.objectName,r=t.propertyName,s=t.propertyIndex;if(e||(e=n.findNode(this.rootNode,t.nodeName),this.node=e),this.getValue=this._getValue_unavailable,this.setValue=this._setValue_unavailable,!e){Ye("PropertyBinding: No target node found for track: "+this.path+".");return}if(i){let c=t.objectIndex;switch(i){case"materials":if(!e.material){Ke("PropertyBinding: Can not bind to material as node does not have a material.",this);return}if(!e.material.materials){Ke("PropertyBinding: Can not bind to material.materials as node.material does not have a materials array.",this);return}e=e.material.materials;break;case"bones":if(!e.skeleton){Ke("PropertyBinding: Can not bind to bones as node does not have a skeleton.",this);return}e=e.skeleton.bones;for(let h=0;h<e.length;h++)if(e[h].name===c){c=h;break}break;case"map":if("map"in e){e=e.map;break}if(!e.material){Ke("PropertyBinding: Can not bind to material as node does not have a material.",this);return}if(!e.material.map){Ke("PropertyBinding: Can not bind to material.map as node.material does not have a map.",this);return}e=e.material.map;break;default:if(e[i]===void 0){Ke("PropertyBinding: Can not bind to objectName of node undefined.",this);return}e=e[i]}if(c!==void 0){if(e[c]===void 0){Ke("PropertyBinding: Trying to bind to objectIndex of objectName, but is undefined.",this,e);return}e=e[c]}}let o=e[r];if(o===void 0){let c=t.nodeName;Ke("PropertyBinding: Trying to update property for track: "+c+"."+r+" but it wasn't found.",e);return}let a=this.Versioning.None;this.targetObject=e,e.isMaterial===!0?a=this.Versioning.NeedsUpdate:e.isObject3D===!0&&(a=this.Versioning.MatrixWorldNeedsUpdate);let l=this.BindingType.Direct;if(s!==void 0){if(r==="morphTargetInfluences"){if(!e.geometry){Ke("PropertyBinding: Can not bind to morphTargetInfluences because node does not have a geometry.",this);return}if(!e.geometry.morphAttributes){Ke("PropertyBinding: Can not bind to morphTargetInfluences because node does not have a geometry.morphAttributes.",this);return}e.morphTargetDictionary[s]!==void 0&&(s=e.morphTargetDictionary[s])}l=this.BindingType.ArrayElement,this.resolvedProperty=o,this.propertyIndex=s}else o.fromArray!==void 0&&o.toArray!==void 0?(l=this.BindingType.HasFromToArray,this.resolvedProperty=o):Array.isArray(o)?(l=this.BindingType.EntireArray,this.resolvedProperty=o):this.propertyName=r;this.getValue=this.GetterByBindingType[l],this.setValue=this.SetterByBindingTypeAndVersioning[l][a]}unbind(){this.node=null,this.getValue=this._getValue_unbound,this.setValue=this._setValue_unbound}};Ft.Composite=Tc;Ft.prototype.BindingType={Direct:0,EntireArray:1,ArrayElement:2,HasFromToArray:3};Ft.prototype.Versioning={None:0,NeedsUpdate:1,MatrixWorldNeedsUpdate:2};Ft.prototype.GetterByBindingType=[Ft.prototype._getValue_direct,Ft.prototype._getValue_array,Ft.prototype._getValue_arrayElement,Ft.prototype._getValue_toArray];Ft.prototype.SetterByBindingTypeAndVersioning=[[Ft.prototype._setValue_direct,Ft.prototype._setValue_direct_setNeedsUpdate,Ft.prototype._setValue_direct_setMatrixWorldNeedsUpdate],[Ft.prototype._setValue_array,Ft.prototype._setValue_array_setNeedsUpdate,Ft.prototype._setValue_array_setMatrixWorldNeedsUpdate],[Ft.prototype._setValue_arrayElement,Ft.prototype._setValue_arrayElement_setNeedsUpdate,Ft.prototype._setValue_arrayElement_setMatrixWorldNeedsUpdate],[Ft.prototype._setValue_fromArray,Ft.prototype._setValue_fromArray_setNeedsUpdate,Ft.prototype._setValue_fromArray_setMatrixWorldNeedsUpdate]];var nb=new Float32Array(1);var Th=new wt,io=class{constructor(e,t,i=0,r=1/0){this.ray=new Yi(e,t),this.near=i,this.far=r,this.camera=null,this.layers=new rs,this.params={Mesh:{},Line:{threshold:1},LOD:{},Points:{threshold:1},Sprite:{}}}set(e,t){this.ray.set(e,t)}setFromCamera(e,t){t.isPerspectiveCamera?(this.ray.origin.setFromMatrixPosition(t.matrixWorld),this.ray.direction.set(e.x,e.y,.5).unproject(t).sub(this.ray.origin).normalize(),this.camera=t):t.isOrthographicCamera?(this.ray.origin.set(e.x,e.y,t.projectionMatrix.elements[14]).unproject(t),this.ray.direction.set(0,0,-1).transformDirection(t.matrixWorld),this.camera=t):Ke("Raycaster: Unsupported camera type: "+t.type)}setFromXRController(e){return Th.identity().extractRotation(e.matrixWorld),this.ray.origin.setFromMatrixPosition(e.matrixWorld),this.ray.direction.set(0,0,-1).applyMatrix4(Th),this}intersectObject(e,t=!0,i=[]){return Ac(e,this,i,t),i.sort(Ah),i}intersectObjects(e,t=!0,i=[]){for(let r=0,s=e.length;r<s;r++)Ac(e[r],this,i,t);return i.sort(Ah),i}};function Ah(n,e){return n.distance-e.distance}function Ac(n,e,t,i){let r=!0;if(n.layers.test(e.layers)&&n.raycast(e,t)===!1&&(r=!1),r===!0&&i===!0){let s=n.children;for(let o=0,a=s.length;o<a;o++)Ac(s[o],e,t,!0)}}var us=class{constructor(e=1,t=0,i=0){this.radius=e,this.phi=t,this.theta=i}set(e,t,i){return this.radius=e,this.phi=t,this.theta=i,this}copy(e){return this.radius=e.radius,this.phi=e.phi,this.theta=e.theta,this}makeSafe(){return this.phi=lt(this.phi,1e-6,Math.PI-1e-6),this}setFromVector3(e){return this.setFromCartesianCoords(e.x,e.y,e.z)}setFromCartesianCoords(e,t,i){return this.radius=Math.sqrt(e*e+t*t+i*i),this.radius===0?(this.theta=0,this.phi=0):(this.theta=Math.atan2(e,i),this.phi=Math.acos(lt(t/this.radius,-1,1))),this}clone(){return new this.constructor().copy(this)}};var Rc=class n{static{n.prototype.isMatrix2=!0}constructor(e,t,i,r){this.elements=[1,0,0,1],e!==void 0&&this.set(e,t,i,r)}identity(){return this.set(1,0,0,1),this}fromArray(e,t=0){for(let i=0;i<4;i++)this.elements[i]=e[i+t];return this}set(e,t,i,r){let s=this.elements;return s[0]=e,s[2]=t,s[1]=i,s[3]=r,this}};var ro=class extends li{constructor(e,t=null){super(),this.object=e,this.domElement=t,this.enabled=!0,this.state=-1,this.keys={},this.mouseButtons={LEFT:null,MIDDLE:null,RIGHT:null},this.touches={ONE:null,TWO:null}}connect(e){this.domElement!==null&&this.disconnect(),this.domElement=e}disconnect(){}dispose(){}update(){}};function nu(n,e,t,i){let r=vm(i);switch(t){case $c:return n*e;case Oa:return n*e/r.components*r.byteLength;case Ba:return n*e/r.components*r.byteLength;case sr:return n*e*2/r.components*r.byteLength;case Ha:return n*e*2/r.components*r.byteLength;case Yc:return n*e*3/r.components*r.byteLength;case Bn:return n*e*4/r.components*r.byteLength;case za:return n*e*4/r.components*r.byteLength;case ao:case lo:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*8;case co:case uo:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*16;case Va:case Xa:return Math.max(n,16)*Math.max(e,8)/4;case Ga:case Wa:return Math.max(n,8)*Math.max(e,8)/2;case qa:case $a:case Za:case Ka:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*8;case Ya:case ho:case Ja:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*16;case ja:return Math.floor((n+3)/4)*Math.floor((e+3)/4)*16;case Qa:return Math.floor((n+4)/5)*Math.floor((e+3)/4)*16;case el:return Math.floor((n+4)/5)*Math.floor((e+4)/5)*16;case tl:return Math.floor((n+5)/6)*Math.floor((e+4)/5)*16;case nl:return Math.floor((n+5)/6)*Math.floor((e+5)/6)*16;case il:return Math.floor((n+7)/8)*Math.floor((e+4)/5)*16;case rl:return Math.floor((n+7)/8)*Math.floor((e+5)/6)*16;case sl:return Math.floor((n+7)/8)*Math.floor((e+7)/8)*16;case ol:return Math.floor((n+9)/10)*Math.floor((e+4)/5)*16;case al:return Math.floor((n+9)/10)*Math.floor((e+5)/6)*16;case ll:return Math.floor((n+9)/10)*Math.floor((e+7)/8)*16;case cl:return Math.floor((n+9)/10)*Math.floor((e+9)/10)*16;case ul:return Math.floor((n+11)/12)*Math.floor((e+9)/10)*16;case hl:return Math.floor((n+11)/12)*Math.floor((e+11)/12)*16;case dl:case fl:case pl:return Math.ceil(n/4)*Math.ceil(e/4)*16;case ml:case gl:return Math.ceil(n/4)*Math.ceil(e/4)*8;case fo:case xl:return Math.ceil(n/4)*Math.ceil(e/4)*16}throw new Error(`Unable to determine texture byte length for ${t} format.`)}function vm(n){switch(n){case On:case Vc:return{byteLength:1,components:1};case fs:case Wc:case hi:return{byteLength:2,components:1};case Ua:case Fa:return{byteLength:2,components:4};case ui:case Na:case Jn:return{byteLength:4,components:1};case Xc:case qc:return{byteLength:4,components:3}}throw new Error(`THREE.TextureUtils: Unknown texture type ${n}.`)}typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("register",{detail:{revision:"186"}}));typeof window<"u"&&(window.__THREE__?Ye("WARNING: Multiple instances of Three.js being imported."):window.__THREE__="186");function Ud(){let n=null,e=!1,t=null,i=null;function r(s,o){i=n.requestAnimationFrame(r),t(s,o)}return{start:function(){e!==!0&&t!==null&&n!==null&&(i=n.requestAnimationFrame(r),e=!0)},stop:function(){n!==null&&n.cancelAnimationFrame(i),e=!1},setAnimationLoop:function(s){t=s},setContext:function(s){n=s}}}function _m(n){let e=new WeakMap;function t(a,l){let c=a.array,h=a.usage,d=c.byteLength,u=n.createBuffer();n.bindBuffer(l,u),n.bufferData(l,c,h),a.onUploadCallback();let p;if(c instanceof Float32Array)p=n.FLOAT;else if(typeof Float16Array<"u"&&c instanceof Float16Array)p=n.HALF_FLOAT;else if(c instanceof Uint16Array)a.isFloat16BufferAttribute?p=n.HALF_FLOAT:p=n.UNSIGNED_SHORT;else if(c instanceof Int16Array)p=n.SHORT;else if(c instanceof Uint32Array)p=n.UNSIGNED_INT;else if(c instanceof Int32Array)p=n.INT;else if(c instanceof Int8Array)p=n.BYTE;else if(c instanceof Uint8Array)p=n.UNSIGNED_BYTE;else if(c instanceof Uint8ClampedArray)p=n.UNSIGNED_BYTE;else throw new Error("THREE.WebGLAttributes: Unsupported buffer data format: "+c);return{buffer:u,type:p,bytesPerElement:c.BYTES_PER_ELEMENT,version:a.version,size:d}}function i(a,l,c){let h=l.array,d=l.updateRanges;if(n.bindBuffer(c,a),d.length===0)n.bufferSubData(c,0,h);else{d.sort((p,x)=>p.start-x.start);let u=0;for(let p=1;p<d.length;p++){let x=d[u],v=d[p];v.start<=x.start+x.count+1?x.count=Math.max(x.count,v.start+v.count-x.start):(++u,d[u]=v)}d.length=u+1;for(let p=0,x=d.length;p<x;p++){let v=d[p];n.bufferSubData(c,v.start*h.BYTES_PER_ELEMENT,h,v.start,v.count)}l.clearUpdateRanges()}l.onUploadCallback()}function r(a){return a.isInterleavedBufferAttribute&&(a=a.data),e.get(a)}function s(a){a.isInterleavedBufferAttribute&&(a=a.data);let l=e.get(a);l&&(n.deleteBuffer(l.buffer),e.delete(a))}function o(a,l){if(a.isInterleavedBufferAttribute&&(a=a.data),a.isGLBufferAttribute){let h=e.get(a);(!h||h.version<a.version)&&e.set(a,{buffer:a.buffer,type:a.type,bytesPerElement:a.elementSize,version:a.version});return}let c=e.get(a);if(c===void 0)e.set(a,t(a,l));else if(c.version<a.version){if(c.size!==a.array.byteLength)throw new Error("THREE.WebGLAttributes: The size of the buffer attribute's array buffer does not match the original size. Resizing buffer attributes is not supported.");i(c.buffer,a,l),c.version=a.version}}return{get:r,remove:s,update:o}}var bm=`#ifdef USE_ALPHAHASH
	if ( diffuseColor.a < getAlphaHashThreshold( vPosition ) ) discard;
#endif`,Mm=`#ifdef USE_ALPHAHASH
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
#endif`,Sm=`#ifdef USE_ALPHAMAP
	diffuseColor.a *= texture2D( alphaMap, vAlphaMapUv ).g;
#endif`,wm=`#ifdef USE_ALPHAMAP
	uniform sampler2D alphaMap;
#endif`,Em=`#ifdef USE_ALPHATEST
	#ifdef ALPHA_TO_COVERAGE
	diffuseColor.a = smoothstep( alphaTest, alphaTest + fwidth( diffuseColor.a ), diffuseColor.a );
	if ( diffuseColor.a == 0.0 ) discard;
	#else
	if ( diffuseColor.a < alphaTest ) discard;
	#endif
#endif`,Tm=`#ifdef USE_ALPHATEST
	uniform float alphaTest;
#endif`,Am=`#ifdef USE_AOMAP
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
#endif`,Rm=`#ifdef USE_AOMAP
	uniform sampler2D aoMap;
	uniform float aoMapIntensity;
#endif`,Cm=`#ifdef USE_BATCHING
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
#endif`,Pm=`#ifdef USE_BATCHING
	mat4 batchingMatrix = getBatchingMatrix( getIndirectIndex( gl_DrawID ) );
#endif`,Im=`vec3 transformed = vec3( position );
#ifdef USE_ALPHAHASH
	vPosition = vec3( position );
#endif`,km=`vec3 objectNormal = vec3( normal );
#ifdef USE_TANGENT
	vec3 objectTangent = vec3( tangent.xyz );
#endif`,Lm=`float G_BlinnPhong_Implicit( ) {
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
} // validated`,Dm=`#ifdef USE_IRIDESCENCE
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
#endif`,Nm=`#ifdef USE_BUMPMAP
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
#endif`,Um=`#if NUM_CLIPPING_PLANES > 0
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
#endif`,Fm=`#if NUM_CLIPPING_PLANES > 0
	varying vec3 vClipPosition;
	uniform vec4 clippingPlanes[ NUM_CLIPPING_PLANES ];
#endif`,Om=`#if NUM_CLIPPING_PLANES > 0
	varying vec3 vClipPosition;
#endif`,Bm=`#if NUM_CLIPPING_PLANES > 0
	vClipPosition = - mvPosition.xyz;
#endif`,Hm=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA )
	diffuseColor *= vColor;
#endif`,zm=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA )
	varying vec4 vColor;
#endif`,Gm=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA ) || defined( USE_INSTANCING_COLOR ) || defined( USE_BATCHING_COLOR )
	varying vec4 vColor;
#endif`,Vm=`#if defined( USE_COLOR ) || defined( USE_COLOR_ALPHA ) || defined( USE_INSTANCING_COLOR ) || defined( USE_BATCHING_COLOR )
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
#endif`,Wm=`#define PI 3.141592653589793
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
} // validated`,Xm=`#ifdef ENVMAP_TYPE_CUBE_UV
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
#endif`,qm=`vec3 transformedNormal = objectNormal;
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
#endif`,$m=`#ifdef USE_DISPLACEMENTMAP
	uniform sampler2D displacementMap;
	uniform float displacementScale;
	uniform float displacementBias;
#endif`,Ym=`#ifdef USE_DISPLACEMENTMAP
	transformed += normalize( objectNormal ) * ( texture2D( displacementMap, vDisplacementMapUv ).x * displacementScale + displacementBias );
#endif`,Zm=`#ifdef USE_EMISSIVEMAP
	vec4 emissiveColor = texture2D( emissiveMap, vEmissiveMapUv );
	#ifdef DECODE_VIDEO_TEXTURE_EMISSIVE
		emissiveColor = sRGBTransferEOTF( emissiveColor );
	#endif
	totalEmissiveRadiance *= emissiveColor.rgb;
#endif`,Km=`#ifdef USE_EMISSIVEMAP
	uniform sampler2D emissiveMap;
#endif`,Jm="gl_FragColor = linearToOutputTexel( gl_FragColor );",jm=`vec4 LinearTransferOETF( in vec4 value ) {
	return value;
}
vec4 sRGBTransferEOTF( in vec4 value ) {
	return vec4( mix( pow( value.rgb * 0.9478672986 + vec3( 0.0521327014 ), vec3( 2.4 ) ), value.rgb * 0.0773993808, vec3( lessThanEqual( value.rgb, vec3( 0.04045 ) ) ) ), value.a );
}
vec4 sRGBTransferOETF( in vec4 value ) {
	return vec4( mix( pow( value.rgb, vec3( 0.41666 ) ) * 1.055 - vec3( 0.055 ), value.rgb * 12.92, vec3( lessThanEqual( value.rgb, vec3( 0.0031308 ) ) ) ), value.a );
}`,Qm=`#ifdef USE_ENVMAP
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
#endif`,e0=`#ifdef USE_ENVMAP
	uniform float envMapIntensity;
	uniform mat3 envMapRotation;
	#ifdef ENVMAP_TYPE_CUBE
		uniform samplerCube envMap;
	#else
		uniform sampler2D envMap;
	#endif
#endif`,t0=`#ifdef USE_ENVMAP
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
#endif`,n0=`#ifdef USE_ENVMAP
	#if defined( USE_BUMPMAP ) || defined( USE_NORMALMAP ) || defined( PHONG ) || defined( LAMBERT )
		#define ENV_WORLDPOS
	#endif
	#ifdef ENV_WORLDPOS
		
		varying vec3 vWorldPosition;
	#else
		varying vec3 vReflect;
		uniform float refractionRatio;
	#endif
#endif`,i0=`#ifdef USE_ENVMAP
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
#endif`,r0=`#ifdef USE_FOG
	vFogDepth = - mvPosition.z;
#endif`,s0=`#ifdef USE_FOG
	varying float vFogDepth;
#endif`,o0=`#ifdef USE_FOG
	#ifdef FOG_EXP2
		float fogFactor = 1.0 - exp( - fogDensity * fogDensity * vFogDepth * vFogDepth );
	#else
		float fogFactor = smoothstep( fogNear, fogFar, vFogDepth );
	#endif
	gl_FragColor.rgb = mix( gl_FragColor.rgb, fogColor, fogFactor );
#endif`,a0=`#ifdef USE_FOG
	uniform vec3 fogColor;
	varying float vFogDepth;
	#ifdef FOG_EXP2
		uniform float fogDensity;
	#else
		uniform float fogNear;
		uniform float fogFar;
	#endif
#endif`,l0=`#ifdef USE_GRADIENTMAP
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
}`,c0=`#ifdef USE_LIGHTMAP
	uniform sampler2D lightMap;
	uniform float lightMapIntensity;
#endif`,u0=`LambertMaterial material;
material.diffuseColor = diffuseColor.rgb;
material.specularStrength = specularStrength;`,h0=`varying vec3 vViewPosition;
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
#define RE_IndirectDiffuse		RE_IndirectDiffuse_Lambert`,d0=`uniform bool receiveShadow;
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
#include <lightprobes_pars_fragment>`,f0=`#ifdef USE_ENVMAP
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
#endif`,p0=`ToonMaterial material;
material.diffuseColor = diffuseColor.rgb;`,m0=`varying vec3 vViewPosition;
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
#define RE_IndirectDiffuse		RE_IndirectDiffuse_Toon`,g0=`BlinnPhongMaterial material;
material.diffuseColor = diffuseColor.rgb;
material.specularColor = specular;
material.specularShininess = shininess;
material.specularStrength = specularStrength;`,x0=`varying vec3 vViewPosition;
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
#define RE_IndirectDiffuse		RE_IndirectDiffuse_BlinnPhong`,v0=`PhysicalMaterial material;
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
#endif`,y0=`uniform sampler2D dfgLUT;
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
}`,_0=`
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
#endif`,b0=`#if defined( RE_IndirectDiffuse )
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
#endif`,M0=`#if defined( RE_IndirectDiffuse )
	#if defined( LAMBERT ) || defined( PHONG )
		irradiance += iblIrradiance;
	#endif
	RE_IndirectDiffuse( irradiance, geometryPosition, geometryNormal, geometryViewDir, geometryClearcoatNormal, material, reflectedLight );
#endif
#if defined( RE_IndirectSpecular )
	RE_IndirectSpecular( radiance, iblIrradiance, clearcoatRadiance, geometryPosition, geometryNormal, geometryViewDir, geometryClearcoatNormal, material, reflectedLight );
#endif`,S0=`#ifdef USE_LIGHT_PROBES_GRID
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
#endif`,w0=`#if defined( USE_LOGARITHMIC_DEPTH_BUFFER )
	gl_FragDepth = vIsPerspective == 0.0 ? gl_FragCoord.z : log2( vFragDepth ) * logDepthBufFC * 0.5;
#endif`,E0=`#if defined( USE_LOGARITHMIC_DEPTH_BUFFER )
	uniform float logDepthBufFC;
	varying float vFragDepth;
	varying float vIsPerspective;
#endif`,T0=`#ifdef USE_LOGARITHMIC_DEPTH_BUFFER
	varying float vFragDepth;
	varying float vIsPerspective;
#endif`,A0=`#ifdef USE_LOGARITHMIC_DEPTH_BUFFER
	vFragDepth = 1.0 + gl_Position.w;
	vIsPerspective = float( isPerspectiveMatrix( projectionMatrix ) );
#endif`,R0=`#ifdef USE_MAP
	vec4 sampledDiffuseColor = texture2D( map, vMapUv );
	#ifdef DECODE_VIDEO_TEXTURE
		sampledDiffuseColor = sRGBTransferEOTF( sampledDiffuseColor );
	#endif
	diffuseColor *= sampledDiffuseColor;
#endif`,C0=`#ifdef USE_MAP
	uniform sampler2D map;
#endif`,P0=`#if defined( USE_MAP ) || defined( USE_ALPHAMAP )
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
#endif`,I0=`#if defined( USE_POINTS_UV )
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
#endif`,k0=`float metalnessFactor = metalness;
#ifdef USE_METALNESSMAP
	vec4 texelMetalness = texture2D( metalnessMap, vMetalnessMapUv );
	metalnessFactor *= texelMetalness.b;
#endif`,L0=`#ifdef USE_METALNESSMAP
	uniform sampler2D metalnessMap;
#endif`,D0=`#ifdef USE_INSTANCING_MORPH
	float morphTargetInfluences[ MORPHTARGETS_COUNT ];
	float morphTargetBaseInfluence = texelFetch( morphTexture, ivec2( 0, gl_InstanceID ), 0 ).r;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		morphTargetInfluences[i] =  texelFetch( morphTexture, ivec2( i + 1, gl_InstanceID ), 0 ).r;
	}
#endif`,N0=`#if defined( USE_MORPHCOLORS )
	vColor *= morphTargetBaseInfluence;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		#if defined( USE_COLOR_ALPHA )
			if ( morphTargetInfluences[ i ] != 0.0 ) vColor += getMorph( gl_VertexID, i, 2 ) * morphTargetInfluences[ i ];
		#elif defined( USE_COLOR )
			if ( morphTargetInfluences[ i ] != 0.0 ) vColor += getMorph( gl_VertexID, i, 2 ).rgb * morphTargetInfluences[ i ];
		#endif
	}
#endif`,U0=`#ifdef USE_MORPHNORMALS
	objectNormal *= morphTargetBaseInfluence;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		if ( morphTargetInfluences[ i ] != 0.0 ) objectNormal += getMorph( gl_VertexID, i, 1 ).xyz * morphTargetInfluences[ i ];
	}
#endif`,F0=`#ifdef USE_MORPHTARGETS
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
#endif`,O0=`#ifdef USE_MORPHTARGETS
	transformed *= morphTargetBaseInfluence;
	for ( int i = 0; i < MORPHTARGETS_COUNT; i ++ ) {
		if ( morphTargetInfluences[ i ] != 0.0 ) transformed += getMorph( gl_VertexID, i, 0 ).xyz * morphTargetInfluences[ i ];
	}
#endif`,B0=`float faceDirection = gl_FrontFacing ? 1.0 : - 1.0;
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
vec3 nonPerturbedNormal = normal;`,H0=`#ifdef USE_NORMALMAP_OBJECTSPACE
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
#endif`,z0=`#ifndef FLAT_SHADED
	varying vec3 vNormal;
	#ifdef USE_TANGENT
		varying vec3 vTangent;
		varying vec3 vBitangent;
	#endif
#endif`,G0=`#ifndef FLAT_SHADED
	varying vec3 vNormal;
	#ifdef USE_TANGENT
		varying vec3 vTangent;
		varying vec3 vBitangent;
	#endif
#endif`,V0=`#ifndef FLAT_SHADED
	vNormal = normalize( transformedNormal );
	#ifdef USE_TANGENT
		vTangent = normalize( transformedTangent );
		vBitangent = normalize( cross( vNormal, vTangent ) * tangent.w );
		#ifdef FLIP_SIDED
			vBitangent = - vBitangent;
		#endif
	#endif
#endif`,W0=`#ifdef USE_NORMALMAP
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
#endif`,X0=`#ifdef USE_CLEARCOAT
	vec3 clearcoatNormal = nonPerturbedNormal;
#endif`,q0=`#ifdef USE_CLEARCOAT_NORMALMAP
	vec3 clearcoatMapN = texture2D( clearcoatNormalMap, vClearcoatNormalMapUv ).xyz * 2.0 - 1.0;
	clearcoatMapN.xy *= clearcoatNormalScale;
	clearcoatNormal = normalize( tbn2 * clearcoatMapN );
#endif`,$0=`#ifdef USE_CLEARCOATMAP
	uniform sampler2D clearcoatMap;
#endif
#ifdef USE_CLEARCOAT_NORMALMAP
	uniform sampler2D clearcoatNormalMap;
	uniform vec2 clearcoatNormalScale;
#endif
#ifdef USE_CLEARCOAT_ROUGHNESSMAP
	uniform sampler2D clearcoatRoughnessMap;
#endif`,Y0=`#ifdef USE_IRIDESCENCEMAP
	uniform sampler2D iridescenceMap;
#endif
#ifdef USE_IRIDESCENCE_THICKNESSMAP
	uniform sampler2D iridescenceThicknessMap;
#endif`,Z0=`#ifdef OPAQUE
diffuseColor.a = 1.0;
#endif
#ifdef USE_TRANSMISSION
diffuseColor.a *= material.transmissionAlpha;
#endif
gl_FragColor = vec4( outgoingLight, diffuseColor.a );`,K0=`vec3 packNormalToRGB( const in vec3 normal ) {
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
}`,J0=`#ifdef PREMULTIPLIED_ALPHA
	gl_FragColor.rgb *= gl_FragColor.a;
#endif`,j0=`vec4 mvPosition = vec4( transformed, 1.0 );
#ifdef USE_BATCHING
	mvPosition = batchingMatrix * mvPosition;
#endif
#ifdef USE_INSTANCING
	mvPosition = instanceMatrix * mvPosition;
#endif
mvPosition = modelViewMatrix * mvPosition;
gl_Position = projectionMatrix * mvPosition;`,Q0=`#ifdef DITHERING
	gl_FragColor.rgb = dithering( gl_FragColor.rgb );
#endif`,eg=`#ifdef DITHERING
	vec3 dithering( vec3 color ) {
		float grid_position = rand( gl_FragCoord.xy );
		vec3 dither_shift_RGB = vec3( 0.25 / 255.0, -0.25 / 255.0, 0.25 / 255.0 );
		dither_shift_RGB = mix( 2.0 * dither_shift_RGB, -2.0 * dither_shift_RGB, grid_position );
		return color + dither_shift_RGB;
	}
#endif`,tg=`float roughnessFactor = roughness;
#ifdef USE_ROUGHNESSMAP
	vec4 texelRoughness = texture2D( roughnessMap, vRoughnessMapUv );
	roughnessFactor *= texelRoughness.g;
#endif`,ng=`#ifdef USE_ROUGHNESSMAP
	uniform sampler2D roughnessMap;
#endif`,ig=`#if NUM_SPOT_LIGHT_COORDS > 0
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
#endif`,rg=`#if NUM_SPOT_LIGHT_COORDS > 0
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
#endif`,sg=`#if ( defined( USE_SHADOWMAP ) && ( NUM_DIR_LIGHT_SHADOWS > 0 || NUM_SUN_LIGHT_SHADOWS > 0 || NUM_POINT_LIGHT_SHADOWS > 0 ) ) || ( NUM_SPOT_LIGHT_COORDS > 0 )
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
#endif`,og=`float getShadowMask() {
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
}`,ag=`#ifdef USE_SKINNING
	mat4 boneMatX = getBoneMatrix( skinIndex.x );
	mat4 boneMatY = getBoneMatrix( skinIndex.y );
	mat4 boneMatZ = getBoneMatrix( skinIndex.z );
	mat4 boneMatW = getBoneMatrix( skinIndex.w );
#endif`,lg=`#ifdef USE_SKINNING
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
#endif`,cg=`#ifdef USE_SKINNING
	vec4 skinVertex = bindMatrix * vec4( transformed, 1.0 );
	vec4 skinned = vec4( 0.0 );
	skinned += boneMatX * skinVertex * skinWeight.x;
	skinned += boneMatY * skinVertex * skinWeight.y;
	skinned += boneMatZ * skinVertex * skinWeight.z;
	skinned += boneMatW * skinVertex * skinWeight.w;
	transformed = ( bindMatrixInverse * skinned ).xyz;
#endif`,ug=`#ifdef USE_SKINNING
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
#endif`,hg=`float specularStrength;
#ifdef USE_SPECULARMAP
	vec4 texelSpecular = texture2D( specularMap, vSpecularMapUv );
	specularStrength = texelSpecular.r;
#else
	specularStrength = 1.0;
#endif`,dg=`#ifdef USE_SPECULARMAP
	uniform sampler2D specularMap;
#endif`,fg=`#if defined( TONE_MAPPING )
	gl_FragColor.rgb = toneMapping( gl_FragColor.rgb );
#endif`,pg=`#ifndef saturate
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
vec3 CustomToneMapping( vec3 color ) { return color; }`,mg=`#ifdef USE_TRANSMISSION
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
#endif`,gg=`#ifdef USE_TRANSMISSION
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
#endif`,xg=`#if defined( USE_UV ) || defined( USE_ANISOTROPY )
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
#endif`,vg=`#if defined( USE_UV ) || defined( USE_ANISOTROPY )
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
#endif`,yg=`#if defined( USE_UV ) || defined( USE_ANISOTROPY )
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
#endif`,_g=`#if defined( USE_ENVMAP ) || defined( DISTANCE ) || defined ( USE_SHADOWMAP ) || defined ( USE_TRANSMISSION ) || NUM_SPOT_LIGHT_COORDS > 0
	vec4 worldPosition = vec4( transformed, 1.0 );
	#ifdef USE_BATCHING
		worldPosition = batchingMatrix * worldPosition;
	#endif
	#ifdef USE_INSTANCING
		worldPosition = instanceMatrix * worldPosition;
	#endif
	worldPosition = modelMatrix * worldPosition;
#endif`,bg=`varying vec2 vUv;
uniform mat3 uvTransform;
void main() {
	vUv = ( uvTransform * vec3( uv, 1 ) ).xy;
	gl_Position = vec4( position.xy, 1.0, 1.0 );
}`,Mg=`uniform sampler2D t2D;
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
}`,Sg=`varying vec3 vWorldDirection;
#include <common>
void main() {
	vWorldDirection = transformDirection( position, modelMatrix );
	#include <begin_vertex>
	#include <project_vertex>
	gl_Position.z = gl_Position.w;
}`,wg=`#ifdef ENVMAP_TYPE_CUBE
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
}`,Eg=`varying vec3 vWorldDirection;
#include <common>
void main() {
	vWorldDirection = transformDirection( position, modelMatrix );
	#include <begin_vertex>
	#include <project_vertex>
	gl_Position.z = gl_Position.w;
}`,Tg=`uniform samplerCube tCube;
uniform float tFlip;
uniform float opacity;
varying vec3 vWorldDirection;
void main() {
	vec4 texColor = textureCube( tCube, vec3( tFlip * vWorldDirection.x, vWorldDirection.yz ) );
	gl_FragColor = texColor;
	gl_FragColor.a *= opacity;
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
}`,Ag=`#include <common>
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
}`,Rg=`#if DEPTH_PACKING == 3200
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
}`,Cg=`#define DISTANCE
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
}`,Pg=`#define DISTANCE
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
}`,Ig=`varying vec3 vWorldDirection;
#include <common>
void main() {
	vWorldDirection = transformDirection( position, modelMatrix );
	#include <begin_vertex>
	#include <project_vertex>
}`,kg=`uniform sampler2D tEquirect;
varying vec3 vWorldDirection;
#include <common>
void main() {
	vec3 direction = normalize( vWorldDirection );
	vec2 sampleUV = equirectUv( direction );
	gl_FragColor = texture2D( tEquirect, sampleUV );
	#include <tonemapping_fragment>
	#include <colorspace_fragment>
}`,Lg=`uniform float scale;
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
}`,Dg=`uniform vec3 diffuse;
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
}`,Ng=`#include <common>
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
}`,Ug=`uniform vec3 diffuse;
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
}`,Fg=`#define LAMBERT
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
}`,Og=`#define LAMBERT
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
}`,Bg=`#define MATCAP
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
}`,Hg=`#define MATCAP
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
}`,zg=`#define NORMAL
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
}`,Gg=`#define NORMAL
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
}`,Vg=`#define PHONG
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
}`,Wg=`#define PHONG
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
}`,Xg=`#define STANDARD
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
}`,qg=`#define STANDARD
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
}`,$g=`#define TOON
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
}`,Yg=`#define TOON
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
}`,Zg=`uniform float size;
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
}`,Kg=`uniform vec3 diffuse;
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
}`,Jg=`#include <common>
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
}`,jg=`uniform vec3 color;
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
}`,Qg=`uniform float rotation;
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
}`,ex=`uniform vec3 diffuse;
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
}`,rt={alphahash_fragment:bm,alphahash_pars_fragment:Mm,alphamap_fragment:Sm,alphamap_pars_fragment:wm,alphatest_fragment:Em,alphatest_pars_fragment:Tm,aomap_fragment:Am,aomap_pars_fragment:Rm,batching_pars_vertex:Cm,batching_vertex:Pm,begin_vertex:Im,beginnormal_vertex:km,bsdfs:Lm,iridescence_fragment:Dm,bumpmap_pars_fragment:Nm,clipping_planes_fragment:Um,clipping_planes_pars_fragment:Fm,clipping_planes_pars_vertex:Om,clipping_planes_vertex:Bm,color_fragment:Hm,color_pars_fragment:zm,color_pars_vertex:Gm,color_vertex:Vm,common:Wm,cube_uv_reflection_fragment:Xm,defaultnormal_vertex:qm,displacementmap_pars_vertex:$m,displacementmap_vertex:Ym,emissivemap_fragment:Zm,emissivemap_pars_fragment:Km,colorspace_fragment:Jm,colorspace_pars_fragment:jm,envmap_fragment:Qm,envmap_common_pars_fragment:e0,envmap_pars_fragment:t0,envmap_pars_vertex:n0,envmap_physical_pars_fragment:f0,envmap_vertex:i0,fog_vertex:r0,fog_pars_vertex:s0,fog_fragment:o0,fog_pars_fragment:a0,gradientmap_pars_fragment:l0,lightmap_pars_fragment:c0,lights_lambert_fragment:u0,lights_lambert_pars_fragment:h0,lights_pars_begin:d0,lights_toon_fragment:p0,lights_toon_pars_fragment:m0,lights_phong_fragment:g0,lights_phong_pars_fragment:x0,lights_physical_fragment:v0,lights_physical_pars_fragment:y0,lights_fragment_begin:_0,lights_fragment_maps:b0,lights_fragment_end:M0,lightprobes_pars_fragment:S0,logdepthbuf_fragment:w0,logdepthbuf_pars_fragment:E0,logdepthbuf_pars_vertex:T0,logdepthbuf_vertex:A0,map_fragment:R0,map_pars_fragment:C0,map_particle_fragment:P0,map_particle_pars_fragment:I0,metalnessmap_fragment:k0,metalnessmap_pars_fragment:L0,morphinstance_vertex:D0,morphcolor_vertex:N0,morphnormal_vertex:U0,morphtarget_pars_vertex:F0,morphtarget_vertex:O0,normal_fragment_begin:B0,normal_fragment_maps:H0,normal_pars_fragment:z0,normal_pars_vertex:G0,normal_vertex:V0,normalmap_pars_fragment:W0,clearcoat_normal_fragment_begin:X0,clearcoat_normal_fragment_maps:q0,clearcoat_pars_fragment:$0,iridescence_pars_fragment:Y0,opaque_fragment:Z0,packing:K0,premultiplied_alpha_fragment:J0,project_vertex:j0,dithering_fragment:Q0,dithering_pars_fragment:eg,roughnessmap_fragment:tg,roughnessmap_pars_fragment:ng,shadowmap_pars_fragment:ig,shadowmap_pars_vertex:rg,shadowmap_vertex:sg,shadowmask_pars_fragment:og,skinbase_vertex:ag,skinning_pars_vertex:lg,skinning_vertex:cg,skinnormal_vertex:ug,specularmap_fragment:hg,specularmap_pars_fragment:dg,tonemapping_fragment:fg,tonemapping_pars_fragment:pg,transmission_fragment:mg,transmission_pars_fragment:gg,uv_pars_fragment:xg,uv_pars_vertex:vg,uv_vertex:yg,worldpos_vertex:_g,background_vert:bg,background_frag:Mg,backgroundCube_vert:Sg,backgroundCube_frag:wg,cube_vert:Eg,cube_frag:Tg,depth_vert:Ag,depth_frag:Rg,distance_vert:Cg,distance_frag:Pg,equirect_vert:Ig,equirect_frag:kg,linedashed_vert:Lg,linedashed_frag:Dg,meshbasic_vert:Ng,meshbasic_frag:Ug,meshlambert_vert:Fg,meshlambert_frag:Og,meshmatcap_vert:Bg,meshmatcap_frag:Hg,meshnormal_vert:zg,meshnormal_frag:Gg,meshphong_vert:Vg,meshphong_frag:Wg,meshphysical_vert:Xg,meshphysical_frag:qg,meshtoon_vert:$g,meshtoon_frag:Yg,points_vert:Zg,points_frag:Kg,shadow_vert:Jg,shadow_frag:jg,sprite_vert:Qg,sprite_frag:ex},Te={common:{diffuse:{value:new ke(16777215)},opacity:{value:1},map:{value:null},mapTransform:{value:new je},alphaMap:{value:null},alphaMapTransform:{value:new je},alphaTest:{value:0}},specularmap:{specularMap:{value:null},specularMapTransform:{value:new je}},envmap:{envMap:{value:null},envMapRotation:{value:new je},reflectivity:{value:1},ior:{value:1.5},refractionRatio:{value:.98},dfgLUT:{value:null}},aomap:{aoMap:{value:null},aoMapIntensity:{value:1},aoMapTransform:{value:new je}},lightmap:{lightMap:{value:null},lightMapIntensity:{value:1},lightMapTransform:{value:new je}},bumpmap:{bumpMap:{value:null},bumpMapTransform:{value:new je},bumpScale:{value:1}},normalmap:{normalMap:{value:null},normalMapTransform:{value:new je},normalScale:{value:new Xe(1,1)}},displacementmap:{displacementMap:{value:null},displacementMapTransform:{value:new je},displacementScale:{value:1},displacementBias:{value:0}},emissivemap:{emissiveMap:{value:null},emissiveMapTransform:{value:new je}},metalnessmap:{metalnessMap:{value:null},metalnessMapTransform:{value:new je}},roughnessmap:{roughnessMap:{value:null},roughnessMapTransform:{value:new je}},gradientmap:{gradientMap:{value:null}},fog:{fogDensity:{value:25e-5},fogNear:{value:1},fogFar:{value:2e3},fogColor:{value:new ke(16777215)}},lights:{ambientLightColor:{value:[]},lightProbe:{value:[]},sunLights:{value:[],properties:{direction:{},color:{}}},sunLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{}}},sunShadowMatrix:{value:[]},sunShadowCascade:{value:[]},directionalLights:{value:[],properties:{direction:{},color:{}}},directionalLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{}}},directionalShadowMatrix:{value:[]},spotLights:{value:[],properties:{color:{},position:{},direction:{},distance:{},coneCos:{},penumbraCos:{},decay:{}}},spotLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{}}},spotLightMap:{value:[]},spotLightMatrix:{value:[]},pointLights:{value:[],properties:{color:{},position:{},decay:{},distance:{}}},pointLightShadows:{value:[],properties:{shadowIntensity:1,shadowBias:{},shadowNormalBias:{},shadowRadius:{},shadowMapSize:{},shadowCameraNear:{},shadowCameraFar:{}}},pointShadowMatrix:{value:[]},hemisphereLights:{value:[],properties:{direction:{},skyColor:{},groundColor:{}}},rectAreaLights:{value:[],properties:{color:{},position:{},width:{},height:{}}},ltc_1:{value:null},ltc_2:{value:null},probesSH:{value:null},probesMin:{value:new G},probesMax:{value:new G},probesResolution:{value:new G}},points:{diffuse:{value:new ke(16777215)},opacity:{value:1},size:{value:1},scale:{value:1},map:{value:null},alphaMap:{value:null},alphaMapTransform:{value:new je},alphaTest:{value:0},uvTransform:{value:new je}},sprite:{diffuse:{value:new ke(16777215)},opacity:{value:1},center:{value:new Xe(.5,.5)},rotation:{value:0},map:{value:null},mapTransform:{value:new je},alphaMap:{value:null},alphaMapTransform:{value:new je},alphaTest:{value:0}}},Mi={basic:{uniforms:Tn([Te.common,Te.specularmap,Te.envmap,Te.aomap,Te.lightmap,Te.fog]),vertexShader:rt.meshbasic_vert,fragmentShader:rt.meshbasic_frag},lambert:{uniforms:Tn([Te.common,Te.specularmap,Te.envmap,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.fog,Te.lights,{emissive:{value:new ke(0)},envMapIntensity:{value:1}}]),vertexShader:rt.meshlambert_vert,fragmentShader:rt.meshlambert_frag},phong:{uniforms:Tn([Te.common,Te.specularmap,Te.envmap,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.fog,Te.lights,{emissive:{value:new ke(0)},specular:{value:new ke(1118481)},shininess:{value:30},envMapIntensity:{value:1}}]),vertexShader:rt.meshphong_vert,fragmentShader:rt.meshphong_frag},standard:{uniforms:Tn([Te.common,Te.envmap,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.roughnessmap,Te.metalnessmap,Te.fog,Te.lights,{emissive:{value:new ke(0)},roughness:{value:1},metalness:{value:0},envMapIntensity:{value:1}}]),vertexShader:rt.meshphysical_vert,fragmentShader:rt.meshphysical_frag},toon:{uniforms:Tn([Te.common,Te.aomap,Te.lightmap,Te.emissivemap,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.gradientmap,Te.fog,Te.lights,{emissive:{value:new ke(0)}}]),vertexShader:rt.meshtoon_vert,fragmentShader:rt.meshtoon_frag},matcap:{uniforms:Tn([Te.common,Te.bumpmap,Te.normalmap,Te.displacementmap,Te.fog,{matcap:{value:null}}]),vertexShader:rt.meshmatcap_vert,fragmentShader:rt.meshmatcap_frag},points:{uniforms:Tn([Te.points,Te.fog]),vertexShader:rt.points_vert,fragmentShader:rt.points_frag},dashed:{uniforms:Tn([Te.common,Te.fog,{scale:{value:1},dashSize:{value:1},totalSize:{value:2}}]),vertexShader:rt.linedashed_vert,fragmentShader:rt.linedashed_frag},depth:{uniforms:Tn([Te.common,Te.displacementmap]),vertexShader:rt.depth_vert,fragmentShader:rt.depth_frag},normal:{uniforms:Tn([Te.common,Te.bumpmap,Te.normalmap,Te.displacementmap,{opacity:{value:1}}]),vertexShader:rt.meshnormal_vert,fragmentShader:rt.meshnormal_frag},sprite:{uniforms:Tn([Te.sprite,Te.fog]),vertexShader:rt.sprite_vert,fragmentShader:rt.sprite_frag},background:{uniforms:{uvTransform:{value:new je},t2D:{value:null},backgroundIntensity:{value:1}},vertexShader:rt.background_vert,fragmentShader:rt.background_frag},backgroundCube:{uniforms:{envMap:{value:null},backgroundBlurriness:{value:0},backgroundIntensity:{value:1},backgroundRotation:{value:new je}},vertexShader:rt.backgroundCube_vert,fragmentShader:rt.backgroundCube_frag},cube:{uniforms:{tCube:{value:null},tFlip:{value:-1},opacity:{value:1}},vertexShader:rt.cube_vert,fragmentShader:rt.cube_frag},equirect:{uniforms:{tEquirect:{value:null}},vertexShader:rt.equirect_vert,fragmentShader:rt.equirect_frag},distance:{uniforms:Tn([Te.common,Te.displacementmap,{referencePosition:{value:new G},nearDistance:{value:1},farDistance:{value:1e3}}]),vertexShader:rt.distance_vert,fragmentShader:rt.distance_frag},shadow:{uniforms:Tn([Te.lights,Te.fog,{color:{value:new ke(0)},opacity:{value:1}}]),vertexShader:rt.shadow_vert,fragmentShader:rt.shadow_frag}};Mi.physical={uniforms:Tn([Mi.standard.uniforms,{clearcoat:{value:0},clearcoatMap:{value:null},clearcoatMapTransform:{value:new je},clearcoatNormalMap:{value:null},clearcoatNormalMapTransform:{value:new je},clearcoatNormalScale:{value:new Xe(1,1)},clearcoatRoughness:{value:0},clearcoatRoughnessMap:{value:null},clearcoatRoughnessMapTransform:{value:new je},dispersion:{value:0},retroreflectivity:{value:0},iridescence:{value:0},iridescenceMap:{value:null},iridescenceMapTransform:{value:new je},iridescenceIOR:{value:1.3},iridescenceThicknessMinimum:{value:100},iridescenceThicknessMaximum:{value:400},iridescenceThicknessMap:{value:null},iridescenceThicknessMapTransform:{value:new je},sheen:{value:0},sheenColor:{value:new ke(0)},sheenColorMap:{value:null},sheenColorMapTransform:{value:new je},sheenRoughness:{value:1},sheenRoughnessMap:{value:null},sheenRoughnessMapTransform:{value:new je},transmission:{value:0},transmissionMap:{value:null},transmissionMapTransform:{value:new je},transmissionSamplerSize:{value:new Xe},transmissionSamplerMap:{value:null},thickness:{value:0},thicknessMap:{value:null},thicknessMapTransform:{value:new je},attenuationDistance:{value:0},attenuationColor:{value:new ke(0)},specularColor:{value:new ke(1,1,1)},specularColorMap:{value:null},specularColorMapTransform:{value:new je},specularIntensity:{value:1},specularIntensityMap:{value:null},specularIntensityMapTransform:{value:new je},anisotropyVector:{value:new Xe},anisotropyMap:{value:null},anisotropyMapTransform:{value:new je}}]),vertexShader:rt.meshphysical_vert,fragmentShader:rt.meshphysical_frag};var Ml={r:0,b:0,g:0},tx=new wt,Fd=new je;Fd.set(-1,0,0,0,1,0,0,0,1);function nx(n,e,t,i,r,s){let o=new ke(0),a=r===!0?0:1,l,c,h=null,d=0,u=null;function p(b){let P=b.isScene===!0?b.background:null;if(P&&P.isTexture){let w=b.backgroundBlurriness>0;P=e.get(P,w)}return P}function x(b){let P=!1,w=p(b);w===null?m(o,a):w&&w.isColor&&(m(w,1),P=!0);let T=n.xr.getEnvironmentBlendMode();T==="additive"?t.buffers.color.setClear(0,0,0,1,s):T==="alpha-blend"&&t.buffers.color.setClear(0,0,0,0,s),(n.autoClear||P)&&(t.buffers.depth.setTest(!0),t.buffers.depth.setMask(!0),t.buffers.color.setMask(!0),n.clear(n.autoClearColor,n.autoClearDepth,n.autoClearStencil))}function v(b,P){let w=p(P);w&&(w.isCubeTexture||w.mapping===so)?(c===void 0&&(c=new $e(new bt(1,1,1),new En({name:"BackgroundCubeMaterial",uniforms:Er(Mi.backgroundCube.uniforms),vertexShader:Mi.backgroundCube.vertexShader,fragmentShader:Mi.backgroundCube.fragmentShader,side:mn,depthTest:!1,depthWrite:!1,fog:!1,allowOverride:!1})),c.geometry.deleteAttribute("normal"),c.geometry.deleteAttribute("uv"),c.onBeforeRender=function(T,E,D){this.matrixWorld.copyPosition(D.matrixWorld)},Object.defineProperty(c.material,"envMap",{get:function(){return this.uniforms.envMap.value}}),i.update(c)),c.material.uniforms.envMap.value=w,c.material.uniforms.backgroundBlurriness.value=P.backgroundBlurriness,c.material.uniforms.backgroundIntensity.value=P.backgroundIntensity,c.material.uniforms.backgroundRotation.value.setFromMatrix4(tx.makeRotationFromEuler(P.backgroundRotation)).transpose(),w.isCubeTexture&&w.isRenderTargetTexture===!1&&c.material.uniforms.backgroundRotation.value.premultiply(Fd),c.material.toneMapped=ht.getTransfer(w.colorSpace)!==St,(h!==w||d!==w.version||u!==n.toneMapping)&&(c.material.needsUpdate=!0,h=w,d=w.version,u=n.toneMapping),c.layers.enableAll(),b.unshift(c,c.geometry,c.material,0,0,null)):w&&w.isTexture&&(l===void 0&&(l=new $e(new Xn(2,2),new En({name:"BackgroundMaterial",uniforms:Er(Mi.background.uniforms),vertexShader:Mi.background.vertexShader,fragmentShader:Mi.background.fragmentShader,side:tr,depthTest:!1,depthWrite:!1,fog:!1,allowOverride:!1})),l.geometry.deleteAttribute("normal"),Object.defineProperty(l.material,"map",{get:function(){return this.uniforms.t2D.value}}),i.update(l)),l.material.uniforms.t2D.value=w,l.material.uniforms.backgroundIntensity.value=P.backgroundIntensity,l.material.toneMapped=ht.getTransfer(w.colorSpace)!==St,w.matrixAutoUpdate===!0&&w.updateMatrix(),l.material.uniforms.uvTransform.value.copy(w.matrix),(h!==w||d!==w.version||u!==n.toneMapping)&&(l.material.needsUpdate=!0,h=w,d=w.version,u=n.toneMapping),l.layers.enableAll(),b.unshift(l,l.geometry,l.material,0,0,null))}function m(b,P){b.getRGB(Ml,Qc(n)),t.buffers.color.setClear(Ml.r,Ml.g,Ml.b,P,s)}function f(){c!==void 0&&(c.geometry.dispose(),c.material.dispose(),c=void 0),l!==void 0&&(l.geometry.dispose(),l.material.dispose(),l=void 0)}return{getClearColor:function(){return o},setClearColor:function(b,P=1){o.set(b),a=P,m(o,a)},getClearAlpha:function(){return a},setClearAlpha:function(b){a=b,m(o,a)},render:x,addToRenderList:v,dispose:f}}function ix(n,e){let t=n.getParameter(n.MAX_VERTEX_ATTRIBS),i={},r=u(null),s=r,o=!1;function a(A,I,k,N,U){let z=!1,Y=d(A,N,k,I);s!==Y&&(s=Y,c(s.object)),z=p(A,N,k,U),z&&x(A,N,k,U),U!==null&&e.update(U,n.ELEMENT_ARRAY_BUFFER),(z||o)&&(o=!1,w(A,I,k,N),U!==null&&n.bindBuffer(n.ELEMENT_ARRAY_BUFFER,e.get(U).buffer))}function l(){return n.createVertexArray()}function c(A){return n.bindVertexArray(A)}function h(A){return n.deleteVertexArray(A)}function d(A,I,k,N){let U=N.wireframe===!0,z=i[I.id];z===void 0&&(z={},i[I.id]=z);let Y=A.isInstancedMesh===!0?A.id:0,ee=z[Y];ee===void 0&&(ee={},z[Y]=ee);let J=ee[k.id];J===void 0&&(J={},ee[k.id]=J);let ie=J[U];return ie===void 0&&(ie=u(l()),J[U]=ie),ie}function u(A){let I=[],k=[],N=[];for(let U=0;U<t;U++)I[U]=0,k[U]=0,N[U]=0;return{geometry:null,program:null,wireframe:!1,newAttributes:I,enabledAttributes:k,attributeDivisors:N,object:A,attributes:{},index:null}}function p(A,I,k,N){let U=s.attributes,z=I.attributes,Y=0,ee=k.getAttributes();for(let J in ee)if(ee[J].location>=0){let ce=U[J],Se=z[J];if(Se===void 0&&(J==="instanceMatrix"&&A.instanceMatrix&&(Se=A.instanceMatrix),J==="instanceColor"&&A.instanceColor&&(Se=A.instanceColor)),ce===void 0||ce.attribute!==Se||Se&&ce.data!==Se.data)return!0;Y++}return s.attributesNum!==Y||s.index!==N}function x(A,I,k,N){let U={},z=I.attributes,Y=0,ee=k.getAttributes();for(let J in ee)if(ee[J].location>=0){let ce=z[J];ce===void 0&&(J==="instanceMatrix"&&A.instanceMatrix&&(ce=A.instanceMatrix),J==="instanceColor"&&A.instanceColor&&(ce=A.instanceColor));let Se={};Se.attribute=ce,ce&&ce.data&&(Se.data=ce.data),U[J]=Se,Y++}s.attributes=U,s.attributesNum=Y,s.index=N}function v(){let A=s.newAttributes;for(let I=0,k=A.length;I<k;I++)A[I]=0}function m(A){f(A,0)}function f(A,I){let k=s.newAttributes,N=s.enabledAttributes,U=s.attributeDivisors;k[A]=1,N[A]===0&&(n.enableVertexAttribArray(A),N[A]=1),U[A]!==I&&(n.vertexAttribDivisor(A,I),U[A]=I)}function b(){let A=s.newAttributes,I=s.enabledAttributes;for(let k=0,N=I.length;k<N;k++)I[k]!==A[k]&&(n.disableVertexAttribArray(k),I[k]=0)}function P(A,I,k,N,U,z,Y){Y===!0?n.vertexAttribIPointer(A,I,k,U,z):n.vertexAttribPointer(A,I,k,N,U,z)}function w(A,I,k,N){v();let U=N.attributes,z=k.getAttributes(),Y=I.defaultAttributeValues;for(let ee in z){let J=z[ee];if(J.location>=0){let ie=U[ee];if(ie===void 0&&(ee==="instanceMatrix"&&A.instanceMatrix&&(ie=A.instanceMatrix),ee==="instanceColor"&&A.instanceColor&&(ie=A.instanceColor)),ie!==void 0){let ce=ie.normalized,Se=ie.itemSize,fe=e.get(ie);if(fe===void 0)continue;let Ie=fe.buffer,We=fe.type,He=fe.bytesPerElement,oe=We===n.INT||We===n.UNSIGNED_INT||ie.gpuType===Na;if(ie.isInterleavedBufferAttribute){let le=ie.data,be=le.stride,Ve=ie.offset;if(le.isInstancedInterleavedBuffer){for(let Me=0;Me<J.locationSize;Me++)f(J.location+Me,le.meshPerAttribute);A.isInstancedMesh!==!0&&N._maxInstanceCount===void 0&&(N._maxInstanceCount=le.meshPerAttribute*le.count)}else for(let Me=0;Me<J.locationSize;Me++)m(J.location+Me);n.bindBuffer(n.ARRAY_BUFFER,Ie);for(let Me=0;Me<J.locationSize;Me++)P(J.location+Me,Se/J.locationSize,We,ce,be*He,(Ve+Se/J.locationSize*Me)*He,oe)}else{if(ie.isInstancedBufferAttribute){for(let le=0;le<J.locationSize;le++)f(J.location+le,ie.meshPerAttribute);A.isInstancedMesh!==!0&&N._maxInstanceCount===void 0&&(N._maxInstanceCount=ie.meshPerAttribute*ie.count)}else for(let le=0;le<J.locationSize;le++)m(J.location+le);n.bindBuffer(n.ARRAY_BUFFER,Ie);for(let le=0;le<J.locationSize;le++)P(J.location+le,Se/J.locationSize,We,ce,Se*He,Se/J.locationSize*le*He,oe)}}else if(Y!==void 0){let ce=Y[ee];if(ce!==void 0)switch(ce.length){case 2:n.vertexAttrib2fv(J.location,ce);break;case 3:n.vertexAttrib3fv(J.location,ce);break;case 4:n.vertexAttrib4fv(J.location,ce);break;default:n.vertexAttrib1fv(J.location,ce)}}}}b()}function T(){S();for(let A in i){let I=i[A];for(let k in I){let N=I[k];for(let U in N){let z=N[U];for(let Y in z)h(z[Y].object),delete z[Y];delete N[U]}}delete i[A]}}function E(A){if(i[A.id]===void 0)return;let I=i[A.id];for(let k in I){let N=I[k];for(let U in N){let z=N[U];for(let Y in z)h(z[Y].object),delete z[Y];delete N[U]}}delete i[A.id]}function D(A){for(let I in i){let k=i[I];for(let N in k){let U=k[N];if(U[A.id]===void 0)continue;let z=U[A.id];for(let Y in z)h(z[Y].object),delete z[Y];delete U[A.id]}}}function g(A){for(let I in i){let k=i[I],N=A.isInstancedMesh===!0?A.id:0,U=k[N];if(U!==void 0){for(let z in U){let Y=U[z];for(let ee in Y)h(Y[ee].object),delete Y[ee];delete U[z]}delete k[N],Object.keys(k).length===0&&delete i[I]}}}function S(){_(),o=!0,s!==r&&(s=r,c(s.object))}function _(){r.geometry=null,r.program=null,r.wireframe=!1}return{setup:a,reset:S,resetDefaultState:_,dispose:T,releaseStatesOfGeometry:E,releaseStatesOfObject:g,releaseStatesOfProgram:D,initAttributes:v,enableAttribute:m,disableUnusedAttributes:b}}function rx(n,e,t){let i;function r(l){i=l}function s(l,c){n.drawArrays(i,l,c),t.update(c,i,1)}function o(l,c,h){h!==0&&(n.drawArraysInstanced(i,l,c,h),t.update(c,i,h))}function a(l,c,h){if(h===0)return;e.get("WEBGL_multi_draw").multiDrawArraysWEBGL(i,l,0,c,0,h);let u=0;for(let p=0;p<h;p++)u+=c[p];t.update(u,i,1)}this.setMode=r,this.render=s,this.renderInstances=o,this.renderMultiDraw=a}function sx(n,e,t,i){let r;function s(){if(r!==void 0)return r;if(e.has("EXT_texture_filter_anisotropic")===!0){let D=e.get("EXT_texture_filter_anisotropic");r=n.getParameter(D.MAX_TEXTURE_MAX_ANISOTROPY_EXT)}else r=0;return r}function o(D){return!(D!==Bn&&i.convert(D)!==n.getParameter(n.IMPLEMENTATION_COLOR_READ_FORMAT))}function a(D){let g=D===hi&&(e.has("EXT_color_buffer_half_float")||e.has("EXT_color_buffer_float"));return!(D!==On&&D!==Jn&&!g&&i.convert(D)!==n.getParameter(n.IMPLEMENTATION_COLOR_READ_TYPE))}function l(D){if(D==="highp"){if(n.getShaderPrecisionFormat(n.VERTEX_SHADER,n.HIGH_FLOAT).precision>0&&n.getShaderPrecisionFormat(n.FRAGMENT_SHADER,n.HIGH_FLOAT).precision>0)return"highp";D="mediump"}return D==="mediump"&&n.getShaderPrecisionFormat(n.VERTEX_SHADER,n.MEDIUM_FLOAT).precision>0&&n.getShaderPrecisionFormat(n.FRAGMENT_SHADER,n.MEDIUM_FLOAT).precision>0?"mediump":"lowp"}let c=t.precision!==void 0?t.precision:"highp",h=l(c);h!==c&&(Ye("WebGLRenderer:",c,"not supported, using",h,"instead."),c=h);let d=t.logarithmicDepthBuffer===!0,u=t.reversedDepthBuffer===!0&&e.has("EXT_clip_control");t.reversedDepthBuffer===!0&&u===!1&&Ye("WebGLRenderer: Unable to use reversed depth buffer due to missing EXT_clip_control extension. Fallback to default depth buffer.");let p=n.getParameter(n.MAX_TEXTURE_IMAGE_UNITS),x=n.getParameter(n.MAX_VERTEX_TEXTURE_IMAGE_UNITS),v=n.getParameter(n.MAX_TEXTURE_SIZE),m=n.getParameter(n.MAX_CUBE_MAP_TEXTURE_SIZE),f=n.getParameter(n.MAX_VERTEX_ATTRIBS),b=n.getParameter(n.MAX_VERTEX_UNIFORM_VECTORS),P=n.getParameter(n.MAX_VARYING_VECTORS),w=n.getParameter(n.MAX_FRAGMENT_UNIFORM_VECTORS),T=n.getParameter(n.MAX_SAMPLES),E=n.getParameter(n.SAMPLES);return{isWebGL2:!0,getMaxAnisotropy:s,getMaxPrecision:l,textureFormatReadable:o,textureTypeReadable:a,precision:c,logarithmicDepthBuffer:d,reversedDepthBuffer:u,maxTextures:p,maxVertexTextures:x,maxTextureSize:v,maxCubemapSize:m,maxAttributes:f,maxVertexUniforms:b,maxVaryings:P,maxFragmentUniforms:w,maxSamples:T,samples:E}}function ox(n){let e=this,t=null,i=0,r=!1,s=!1,o=new Wn,a=new je,l={value:null,needsUpdate:!1};this.uniform=l,this.numPlanes=0,this.numIntersection=0,this.init=function(d,u){let p=d.length!==0||u||i!==0||r;return r=u,i=d.length,p},this.beginShadows=function(){s=!0,h(null)},this.endShadows=function(){s=!1},this.setGlobalState=function(d,u){t=h(d,u,0)},this.setState=function(d,u,p){let x=d.clippingPlanes,v=d.clipIntersection,m=d.clipShadows,f=n.get(d);if(!r||x===null||x.length===0||s&&!m)s?h(null):c();else{let b=s?0:i,P=b*4,w=f.clippingState||null;l.value=w,w=h(x,u,P,p);for(let T=0;T!==P;++T)w[T]=t[T];f.clippingState=w,this.numIntersection=v?this.numPlanes:0,this.numPlanes+=b}};function c(){l.value!==t&&(l.value=t,l.needsUpdate=i>0),e.numPlanes=i,e.numIntersection=0}function h(d,u,p,x){let v=d!==null?d.length:0,m=null;if(v!==0){if(m=l.value,x!==!0||m===null){let f=p+v*4,b=u.matrixWorldInverse;a.getNormalMatrix(b),(m===null||m.length<f)&&(m=new Float32Array(f));for(let P=0,w=p;P!==v;++P,w+=4)o.copy(d[P]).applyMatrix4(b,a),o.normal.toArray(m,w),m[w+3]=o.constant}l.value=m,l.needsUpdate=!0}return e.numPlanes=v,e.numIntersection=0,m}}var xs=4,ax=6,lx=20,cx=256,po=new cs,md=new ke,iu=null,ru=0,su=0,ou=!1,ux=new G,Tr=new G,wl=class{constructor(e){this._renderer=e,this._pingPongRenderTarget=null,this._lodMax=0,this._cubeSize=0,this._sizeLods=[],this._lodMeshes=[],this._backgroundBox=null,this._cubemapMaterial=null,this._equirectMaterial=null,this._blurMaterial=null,this._ggxMaterial=null}fromScene(e,t=0,i=.1,r=100,s={}){let{size:o=256,position:a=ux}=s;iu=this._renderer.getRenderTarget(),ru=this._renderer.getActiveCubeFace(),su=this._renderer.getActiveMipmapLevel(),ou=this._renderer.xr.enabled,this._renderer.xr.enabled=!1,this._setSize(o);let l=this._allocateTargets();return l.depthBuffer=!0,this._sceneToCubeUV(e,i,r,l,a),t>0&&this._blur(l,0,0,t),this._applyPMREM(l),this._cleanup(l),l}fromEquirectangular(e,t=null){return this._fromTexture(e,t)}fromCubemap(e,t=null){return this._fromTexture(e,t)}compileCubemapShader(){this._cubemapMaterial===null&&(this._cubemapMaterial=vd(),this._compileMaterial(this._cubemapMaterial))}compileEquirectangularShader(){this._equirectMaterial===null&&(this._equirectMaterial=xd(),this._compileMaterial(this._equirectMaterial))}dispose(){this._dispose(),this._cubemapMaterial!==null&&this._cubemapMaterial.dispose(),this._equirectMaterial!==null&&this._equirectMaterial.dispose(),this._backgroundBox!==null&&(this._backgroundBox.geometry.dispose(),this._backgroundBox.material.dispose())}_setSize(e){this._lodMax=Math.floor(Math.log2(e)),this._cubeSize=Math.pow(2,this._lodMax)}_dispose(){this._blurMaterial!==null&&this._blurMaterial.dispose(),this._ggxMaterial!==null&&this._ggxMaterial.dispose(),this._pingPongRenderTarget!==null&&this._pingPongRenderTarget.dispose();for(let e=0;e<this._lodMeshes.length;e++)this._lodMeshes[e].geometry.dispose()}_cleanup(e){this._renderer.setRenderTarget(iu,ru,su),this._renderer.xr.enabled=ou,e.scissorTest=!1,gs(e,0,0,e.width,e.height)}_fromTexture(e,t){e.mapping===nr||e.mapping===wr?this._setSize(e.image.length===0?16:e.image[0].width||e.image[0].image.width):this._setSize(e.image.width/4),iu=this._renderer.getRenderTarget(),ru=this._renderer.getActiveCubeFace(),su=this._renderer.getActiveMipmapLevel(),ou=this._renderer.xr.enabled,this._renderer.xr.enabled=!1;let i=t||this._allocateTargets();return this._textureToCubeUV(e,i),this._applyPMREM(i),this._cleanup(i),i}_allocateTargets(){let e=3*Math.max(this._cubeSize,112),t=4*this._cubeSize,i={magFilter:vn,minFilter:vn,generateMipmaps:!1,type:hi,format:Bn,colorSpace:Os,depthBuffer:!1},r=gd(e,t,i);if(this._pingPongRenderTarget===null||this._pingPongRenderTarget.width!==e||this._pingPongRenderTarget.height!==t){this._pingPongRenderTarget!==null&&this._dispose(),this._pingPongRenderTarget=gd(e,t,i);let{_lodMax:s}=this;({lodMeshes:this._lodMeshes,sizeLods:this._sizeLods}=hx(s)),this._blurMaterial=fx(s,e,t),this._ggxMaterial=dx(s,e,t)}return r}_compileMaterial(e){let t=new $e(new Yt,e);this._renderer.compile(t,po)}_sceneToCubeUV(e,t,i,r,s){let l=new cn(90,1,t,i),c=[1,-1,1,1,1,1],h=[1,1,1,-1,-1,-1],d=this._renderer,u=d.autoClear,p=d.toneMapping;d.getClearColor(md),d.toneMapping=ci,d.autoClear=!1,d.state.buffers.depth.getReversed()&&(d.setRenderTarget(r),d.clearDepth(),d.setRenderTarget(null)),this._backgroundBox===null&&(this._backgroundBox=new $e(new bt,new Ot({name:"PMREM.Background",side:mn,depthWrite:!1,depthTest:!1})));let v=this._backgroundBox,m=v.material,f=!1,b=e.background;b?b.isColor&&(m.color.copy(b),e.background=null,f=!0):(m.color.copy(md),f=!0);for(let P=0;P<6;P++){let w=P%3;w===0?(l.up.set(0,c[P],0),l.position.set(s.x,s.y,s.z),l.lookAt(s.x+h[P],s.y,s.z)):w===1?(l.up.set(0,0,c[P]),l.position.set(s.x,s.y,s.z),l.lookAt(s.x,s.y+h[P],s.z)):(l.up.set(0,c[P],0),l.position.set(s.x,s.y,s.z),l.lookAt(s.x,s.y,s.z+h[P]));let T=this._cubeSize;gs(r,w*T,P>2?T:0,T,T),d.setRenderTarget(r),f&&d.render(v,l),d.render(e,l)}d.toneMapping=p,d.autoClear=u,e.background=b}_textureToCubeUV(e,t){let i=this._renderer,r=e.mapping===nr||e.mapping===wr;r?(this._cubemapMaterial===null&&(this._cubemapMaterial=vd()),this._cubemapMaterial.uniforms.flipEnvMap.value=e.isRenderTargetTexture===!1?-1:1):this._equirectMaterial===null&&(this._equirectMaterial=xd());let s=r?this._cubemapMaterial:this._equirectMaterial,o=this._lodMeshes[0];o.material=s;let a=s.uniforms;a.envMap.value=e;let l=this._cubeSize;gs(t,0,0,3*l,2*l),i.setRenderTarget(t),i.render(o,po)}_applyPMREM(e){let t=this._renderer,i=t.autoClear;t.autoClear=!1;let r=this._lodMeshes.length;for(let s=1;s<r;s++)this._applyGGXFilter(e,s-1,s);t.autoClear=i}_applyGGXFilter(e,t,i){let r=this._renderer,s=this._pingPongRenderTarget,o=this._ggxMaterial,a=this._lodMeshes[i];a.material=o;let l=o.uniforms,c=i/(this._lodMeshes.length-1),h=t/(this._lodMeshes.length-1),d=Math.sqrt(c*c-h*h),u=c*1.25,p=d*u,{_lodMax:x}=this,v=this._sizeLods[i],m=3*v*(i>x-xs?i-x+xs:0),f=4*(this._cubeSize-v);l.envMap.value=e.texture,l.roughness.value=p,l.mipInt.value=x-t,gs(s,m,f,3*v,2*v),r.setRenderTarget(s),r.render(a,po),l.envMap.value=s.texture,l.roughness.value=0,l.mipInt.value=x-i,gs(e,m,f,3*v,2*v),r.setRenderTarget(e),r.render(a,po)}_blur(e,t,i,r){let s=this._pingPongRenderTarget,o=Math.min(r,Math.PI)/Math.SQRT2;this._blurPass(e,s,t,i,o),this._blurPass(s,e,i,i,o)}_blurPass(e,t,i,r,s){let o=this._renderer,a=this._blurMaterial,l=this._lodMeshes[r];l.material=a;let c=a.uniforms;c.envMap.value=e.texture,c.sigma.value=s,c.mipInt.value=this._lodMax-i;let h=this._sizeLods[r],d=3*h*(r>this._lodMax-xs?r-this._lodMax+xs:0),u=4*(this._cubeSize-h);gs(t,d,u,3*h,2*h),o.setRenderTarget(t),o.render(l,po)}};function hx(n){let e=[],t=[],i=n,r=n-xs+1+ax;for(let s=0;s<r;s++){let o=Math.pow(2,i);e.push(o);let a=1/(o-2),l=-a,c=1+a,h=[l,l,c,l,c,c,l,l,c,c,l,c],d=6,u=6,p=3,x=new Float32Array(p*u*d),v=new Float32Array(p*u*d);for(let f=0;f<d;f++){let b=f%3*2/3-1,P=f>2?0:-1,w=[b,P,0,b+2/3,P,0,b+2/3,P+1,0,b,P,0,b+2/3,P+1,0,b,P+1,0];x.set(w,p*u*f);for(let T=0;T<u;T++){let E=h[T*2]*2-1,D=h[T*2+1]*2-1;f===0?Tr.set(1,D,E):f===1?Tr.set(-E,1,-D):f===2?Tr.set(-E,D,1):f===3?Tr.set(-1,D,-E):f===4?Tr.set(-E,-1,D):Tr.set(E,D,-1),Tr.toArray(v,(f*u+T)*p)}}let m=new Yt;m.setAttribute("position",new Cn(x,p)),m.setAttribute("outputDirection",new Cn(v,p)),t.push(new $e(m,null)),i>xs&&i--}return{lodMeshes:t,sizeLods:e}}function gd(n,e,t){let i=new Dn(n,e,t);return i.texture.mapping=so,i.texture.name="PMREM.cubeUv",i.scissorTest=!0,i}function gs(n,e,t,i,r){n.viewport.set(e,t,i,r),n.scissor.set(e,t,i,r)}function dx(n,e,t){return new En({name:"PMREMGGXConvolution",defines:{GGX_SAMPLES:cx,CUBEUV_TEXEL_WIDTH:1/e,CUBEUV_TEXEL_HEIGHT:1/t,CUBEUV_MAX_MIP:`${n}.0`},uniforms:{envMap:{value:null},roughness:{value:0},mipInt:{value:0}},vertexShader:Tl(),fragmentShader:`

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
		`,blending:_i,depthTest:!1,depthWrite:!1})}function fx(n,e,t){return new En({name:"SphericalGaussianBlur",defines:{SAMPLES:lx,CUBEUV_TEXEL_WIDTH:1/e,CUBEUV_TEXEL_HEIGHT:1/t,CUBEUV_MAX_MIP:`${n}.0`},uniforms:{envMap:{value:null},sigma:{value:0},mipInt:{value:0}},vertexShader:Tl(),fragmentShader:`

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
		`,blending:_i,depthTest:!1,depthWrite:!1})}function xd(){return new En({name:"EquirectangularToCubeUV",uniforms:{envMap:{value:null}},vertexShader:Tl(),fragmentShader:`

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
		`,blending:_i,depthTest:!1,depthWrite:!1})}function vd(){return new En({name:"CubemapToCubeUV",uniforms:{envMap:{value:null},flipEnvMap:{value:-1}},vertexShader:Tl(),fragmentShader:`

			precision mediump float;
			precision mediump int;

			uniform float flipEnvMap;

			varying vec3 vOutputDirection;

			uniform samplerCube envMap;

			void main() {

				gl_FragColor = textureCube( envMap, vec3( flipEnvMap * vOutputDirection.x, vOutputDirection.yz ) );

			}
		`,blending:_i,depthTest:!1,depthWrite:!1})}function Tl(){return`

		precision mediump float;
		precision mediump int;

		attribute vec3 outputDirection;

		varying vec3 vOutputDirection;

		void main() {

			vOutputDirection = outputDirection;
			gl_Position = vec4( position, 1.0 );

		}
	`}var El=class extends Dn{constructor(e=1,t={}){super(e,e,t),this.isWebGLCubeRenderTarget=!0;let i={width:e,height:e,depth:1},r=[i,i,i,i,i,i];this.texture=new Ys(r),this._setTextureOptions(t),this.texture.isRenderTargetTexture=!0}fromEquirectangularTexture(e,t){this.texture.type=t.type,this.texture.colorSpace=t.colorSpace,this.texture.generateMipmaps=t.generateMipmaps,this.texture.minFilter=t.minFilter,this.texture.magFilter=t.magFilter;let i={uniforms:{tEquirect:{value:null}},vertexShader:`

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
			`},r=new bt(5,5,5),s=new En({name:"CubemapFromEquirect",uniforms:Er(i.uniforms),vertexShader:i.vertexShader,fragmentShader:i.fragmentShader,side:mn,blending:_i});s.uniforms.tEquirect.value=t;let o=new $e(r,s),a=t.minFilter;return t.minFilter===ir&&(t.minFilter=vn),new Pa(1,10,this).update(e,o),t.minFilter=a,o.geometry.dispose(),o.material.dispose(),this}clear(e,t=!0,i=!0,r=!0){let s=e.getRenderTarget();for(let o=0;o<6;o++)e.setRenderTarget(this,o),e.clear(t,i,r);e.setRenderTarget(s)}};function px(n){let e=new WeakMap,t=new WeakMap,i=null;function r(u,p=!1){return u==null?null:p?o(u):s(u)}function s(u){if(u&&u.isTexture){let p=u.mapping;if(p===ka||p===La)if(e.has(u)){let x=e.get(u).texture;return a(x,u.mapping)}else{let x=u.image;if(x&&x.height>0){let v=new El(x.height);return v.fromEquirectangularTexture(n,u),e.set(u,v),u.addEventListener("dispose",c),a(v.texture,u.mapping)}else return null}}return u}function o(u){if(u&&u.isTexture){let p=u.mapping,x=p===ka||p===La,v=p===nr||p===wr;if(x||v){let m=t.get(u),f=m!==void 0?m.texture.pmremVersion:0;if(u.isRenderTargetTexture&&u.pmremVersion!==f)return i===null&&(i=new wl(n)),m=x?i.fromEquirectangular(u,m):i.fromCubemap(u,m),m.texture.pmremVersion=u.pmremVersion,t.set(u,m),m.texture;if(m!==void 0)return m.texture;{let b=u.image;return x&&b&&b.height>0||v&&b&&l(b)?(i===null&&(i=new wl(n)),m=x?i.fromEquirectangular(u):i.fromCubemap(u),m.texture.pmremVersion=u.pmremVersion,t.set(u,m),u.addEventListener("dispose",h),m.texture):null}}}return u}function a(u,p){return p===ka?u.mapping=nr:p===La&&(u.mapping=wr),u}function l(u){let p=0,x=6;for(let v=0;v<x;v++)u[v]!==void 0&&p++;return p===x}function c(u){let p=u.target;p.removeEventListener("dispose",c);let x=e.get(p);x!==void 0&&(e.delete(p),x.dispose())}function h(u){let p=u.target;p.removeEventListener("dispose",h);let x=t.get(p);x!==void 0&&(t.delete(p),x.dispose())}function d(){e=new WeakMap,t=new WeakMap,i!==null&&(i.dispose(),i=null)}return{get:r,dispose:d}}function mx(n){let e={};function t(i){if(e[i]!==void 0)return e[i];let r=n.getExtension(i);return e[i]=r,r}return{has:function(i){return t(i)!==null},init:function(){t("EXT_color_buffer_float"),t("WEBGL_clip_cull_distance"),t("OES_texture_float_linear"),t("EXT_color_buffer_half_float"),t("WEBGL_multisampled_render_to_texture"),t("WEBGL_render_shared_exponent")},get:function(i){let r=t(i);return r===null&&mr("WebGLRenderer: "+i+" extension not supported."),r}}}function gx(n,e,t,i){let r={},s=new WeakMap;function o(d){let u=d.target;u.index!==null&&e.remove(u.index);for(let x in u.attributes)e.remove(u.attributes[x]);u.removeEventListener("dispose",o),delete r[u.id];let p=s.get(u);p&&(e.remove(p),s.delete(u)),i.releaseStatesOfGeometry(u),u.isInstancedBufferGeometry===!0&&delete u._maxInstanceCount,t.memory.geometries--}function a(d,u){return r[u.id]===!0||(u.addEventListener("dispose",o),r[u.id]=!0,t.memory.geometries++),u}function l(d){let u=d.attributes;for(let p in u)e.update(u[p],n.ARRAY_BUFFER)}function c(d){let u=[],p=d.index,x=d.attributes.position,v=0;if(x===void 0)return;if(p!==null){let b=p.array;v=p.version;for(let P=0,w=b.length;P<w;P+=3){let T=b[P+0],E=b[P+1],D=b[P+2];u.push(T,E,E,D,D,T)}}else{let b=x.array;v=x.version;for(let P=0,w=b.length/3-1;P<w;P+=3){let T=P+0,E=P+1,D=P+2;u.push(T,E,E,D,D,T)}}let m=new(x.count>=65535?Ws:Vs)(u,1);m.version=v;let f=s.get(d);f&&e.remove(f),s.set(d,m)}function h(d){let u=s.get(d);if(u){let p=d.index;p!==null&&u.version<p.version&&c(d)}else c(d);return s.get(d)}return{get:a,update:l,getWireframeAttribute:h}}function xx(n,e,t){let i;function r(d){i=d}let s,o;function a(d){s=d.type,o=d.bytesPerElement}function l(d,u){n.drawElements(i,u,s,d*o),t.update(u,i,1)}function c(d,u,p){p!==0&&(n.drawElementsInstanced(i,u,s,d*o,p),t.update(u,i,p))}function h(d,u,p){if(p===0)return;e.get("WEBGL_multi_draw").multiDrawElementsWEBGL(i,u,0,s,d,0,p);let v=0;for(let m=0;m<p;m++)v+=u[m];t.update(v,i,1)}this.setMode=r,this.setIndex=a,this.render=l,this.renderInstances=c,this.renderMultiDraw=h}function vx(n){let e={geometries:0,textures:0},t={frame:0,calls:0,triangles:0,points:0,lines:0};function i(s,o,a){switch(t.calls++,o){case n.TRIANGLES:t.triangles+=a*(s/3);break;case n.LINES:t.lines+=a*(s/2);break;case n.LINE_STRIP:t.lines+=a*(s-1);break;case n.LINE_LOOP:t.lines+=a*s;break;case n.POINTS:t.points+=a*s;break;default:Ke("WebGLInfo: Unknown draw mode:",o);break}}function r(){t.calls=0,t.triangles=0,t.points=0,t.lines=0}return{memory:e,render:t,programs:null,autoReset:!0,reset:r,update:i}}function yx(n,e,t){let i=new WeakMap,r=new Vt;function s(o,a,l){let c=o.morphTargetInfluences,h=a.morphAttributes.position||a.morphAttributes.normal||a.morphAttributes.color,d=h!==void 0?h.length:0,u=i.get(a);if(u===void 0||u.count!==d){let S=function(){D.dispose(),i.delete(a),a.removeEventListener("dispose",S)};u!==void 0&&u.texture.dispose();let p=a.morphAttributes.position!==void 0,x=a.morphAttributes.normal!==void 0,v=a.morphAttributes.color!==void 0,m=a.morphAttributes.position||[],f=a.morphAttributes.normal||[],b=a.morphAttributes.color||[],P=0;p===!0&&(P=1),x===!0&&(P=2),v===!0&&(P=3);let w=a.attributes.position.count*P,T=1;w>e.maxTextureSize&&(T=Math.ceil(w/e.maxTextureSize),w=e.maxTextureSize);let E=new Float32Array(w*T*4*d),D=new zs(E,w,T,d);D.type=Jn,D.needsUpdate=!0;let g=P*4;for(let _=0;_<d;_++){let A=m[_],I=f[_],k=b[_],N=w*T*4*_;for(let U=0;U<A.count;U++){let z=U*g;p===!0&&(r.fromBufferAttribute(A,U),E[N+z+0]=r.x,E[N+z+1]=r.y,E[N+z+2]=r.z,E[N+z+3]=0),x===!0&&(r.fromBufferAttribute(I,U),E[N+z+4]=r.x,E[N+z+5]=r.y,E[N+z+6]=r.z,E[N+z+7]=0),v===!0&&(r.fromBufferAttribute(k,U),E[N+z+8]=r.x,E[N+z+9]=r.y,E[N+z+10]=r.z,E[N+z+11]=k.itemSize===4?r.w:1)}}u={count:d,texture:D,size:new Xe(w,T)},i.set(a,u),a.addEventListener("dispose",S)}if(o.isInstancedMesh===!0&&o.morphTexture!==null)l.getUniforms().setValue(n,"morphTexture",o.morphTexture,t);else{let p=0;for(let v=0;v<c.length;v++)p+=c[v];let x=a.morphTargetsRelative?1:1-p;l.getUniforms().setValue(n,"morphTargetBaseInfluence",x),l.getUniforms().setValue(n,"morphTargetInfluences",c)}l.getUniforms().setValue(n,"morphTargetsTexture",u.texture,t),l.getUniforms().setValue(n,"morphTargetsTextureSize",u.size)}return{update:s}}function _x(n,e,t,i,r){let s=new WeakMap;function o(c){let h=r.render.frame,d=c.geometry,u=e.get(c,d);if(s.get(u)!==h&&(e.update(u),s.set(u,h)),c.isInstancedMesh&&(c.hasEventListener("dispose",l)===!1&&c.addEventListener("dispose",l),s.get(c)!==h&&(t.update(c.instanceMatrix,n.ARRAY_BUFFER),c.instanceColor!==null&&t.update(c.instanceColor,n.ARRAY_BUFFER),s.set(c,h))),c.isSkinnedMesh){let p=c.skeleton;s.get(p)!==h&&(p.update(),s.set(p,h))}return u}function a(){s=new WeakMap}function l(c){let h=c.target;h.removeEventListener("dispose",l),i.releaseStatesOfObject(h),t.remove(h.instanceMatrix),h.instanceColor!==null&&t.remove(h.instanceColor)}return{update:o,dispose:a}}var bx={[Nc]:"LINEAR_TONE_MAPPING",[Uc]:"REINHARD_TONE_MAPPING",[Fc]:"CINEON_TONE_MAPPING",[Oc]:"ACES_FILMIC_TONE_MAPPING",[Hc]:"AGX_TONE_MAPPING",[zc]:"NEUTRAL_TONE_MAPPING",[Bc]:"CUSTOM_TONE_MAPPING"};function Mx(n,e,t,i,r,s){let o=new Dn(e,t,{type:n,depthBuffer:r,stencilBuffer:s,samples:i?4:0,storeMultisampledDepthBuffer:!1,storeMultisampledStencilBuffer:!1,resolveDepthBuffer:!1,resolveStencilBuffer:!1}),a=null,l=null,c=new Yt;c.setAttribute("position",new _t([-1,3,0,-1,-1,0,3,-1,0],3)),c.setAttribute("uv",new _t([0,2,0,0,2,0],2));let h=new xa({uniforms:{tDiffuse:{value:null}},vertexShader:`
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
			}`,depthTest:!1,depthWrite:!1}),d=new $e(c,h),u=new cs(-1,1,1,-1,0,1),p=null,x=null,v=!1,m,f=null,b=[],P=!1;this.setSize=function(w,T){o.setSize(w,T),a!==null&&a.setSize(w,T),l!==null&&l.setSize(w,T);for(let E=0;E<b.length;E++){let D=b[E];D.setSize&&D.setSize(w,T)}},this.setEffects=function(w){b=w,P=b.length>0&&b[0].isRenderPass===!0;let T=o.width,E=o.height;b.length>0&&a===null&&(a=new Dn(T,E,{type:hi,depthBuffer:!1,stencilBuffer:!1}),l=new Dn(T,E,{type:hi,depthBuffer:!1,stencilBuffer:!1}));for(let D=0;D<b.length;D++){let g=b[D];g.setSize&&g.setSize(T,E)}},this.begin=function(w,T){if(v||w.toneMapping===ci&&b.length===0)return!1;if(f=T,T!==null){let E=T.width,D=T.height;(o.width!==E||o.height!==D)&&this.setSize(E,D)}return P===!1&&w.setRenderTarget(o),m=w.toneMapping,w.toneMapping=ci,!0},this.hasRenderPass=function(){return P},this.end=function(w,T){w.toneMapping=m,v=!0;let E=o,D=a;for(let g=0;g<b.length;g++){let S=b[g];S.enabled!==!1&&(S.render(w,D,E,T),S.needsSwap!==!1&&(E=D,D=D===a?l:a))}if(p!==w.outputColorSpace||x!==w.toneMapping){p=w.outputColorSpace,x=w.toneMapping,h.defines={},ht.getTransfer(p)===St&&(h.defines.SRGB_TRANSFER="");let g=bx[x];g&&(h.defines[g]=""),h.needsUpdate=!0}h.uniforms.tDiffuse.value=E.texture,w.setRenderTarget(f),w.render(d,u),f=null,v=!1},this.isCompositing=function(){return v},this.dispose=function(){o.dispose(),a!==null&&a.dispose(),l!==null&&l.dispose(),c.dispose(),h.dispose()}}var Od=new Pn,cu=new Zi(1,1),Bd=new zs,Hd=new pa,zd=new Ys,yd=[],_d=[],bd=new Float32Array(16),Md=new Float32Array(9),Sd=new Float32Array(4);function _s(n,e,t){let i=n[0];if(i<=0||i>0)return n;let r=e*t,s=yd[r];if(s===void 0&&(s=new Float32Array(r),yd[r]=s),e!==0){i.toArray(s,0);for(let o=1,a=0;o!==e;++o)a+=t,n[o].toArray(s,a)}return s}function hn(n,e){if(n.length!==e.length)return!1;for(let t=0,i=n.length;t<i;t++)if(n[t]!==e[t])return!1;return!0}function dn(n,e){for(let t=0,i=e.length;t<i;t++)n[t]=e[t]}function Al(n,e){let t=_d[e];t===void 0&&(t=new Int32Array(e),_d[e]=t);for(let i=0;i!==e;++i)t[i]=n.allocateTextureUnit();return t}function Sx(n,e){let t=this.cache;t[0]!==e&&(n.uniform1f(this.addr,e),t[0]=e)}function wx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y)&&(n.uniform2f(this.addr,e.x,e.y),t[0]=e.x,t[1]=e.y);else{if(hn(t,e))return;n.uniform2fv(this.addr,e),dn(t,e)}}function Ex(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z)&&(n.uniform3f(this.addr,e.x,e.y,e.z),t[0]=e.x,t[1]=e.y,t[2]=e.z);else if(e.r!==void 0)(t[0]!==e.r||t[1]!==e.g||t[2]!==e.b)&&(n.uniform3f(this.addr,e.r,e.g,e.b),t[0]=e.r,t[1]=e.g,t[2]=e.b);else{if(hn(t,e))return;n.uniform3fv(this.addr,e),dn(t,e)}}function Tx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z||t[3]!==e.w)&&(n.uniform4f(this.addr,e.x,e.y,e.z,e.w),t[0]=e.x,t[1]=e.y,t[2]=e.z,t[3]=e.w);else{if(hn(t,e))return;n.uniform4fv(this.addr,e),dn(t,e)}}function Ax(n,e){let t=this.cache,i=e.elements;if(i===void 0){if(hn(t,e))return;n.uniformMatrix2fv(this.addr,!1,e),dn(t,e)}else{if(hn(t,i))return;Sd.set(i),n.uniformMatrix2fv(this.addr,!1,Sd),dn(t,i)}}function Rx(n,e){let t=this.cache,i=e.elements;if(i===void 0){if(hn(t,e))return;n.uniformMatrix3fv(this.addr,!1,e),dn(t,e)}else{if(hn(t,i))return;Md.set(i),n.uniformMatrix3fv(this.addr,!1,Md),dn(t,i)}}function Cx(n,e){let t=this.cache,i=e.elements;if(i===void 0){if(hn(t,e))return;n.uniformMatrix4fv(this.addr,!1,e),dn(t,e)}else{if(hn(t,i))return;bd.set(i),n.uniformMatrix4fv(this.addr,!1,bd),dn(t,i)}}function Px(n,e){let t=this.cache;t[0]!==e&&(n.uniform1i(this.addr,e),t[0]=e)}function Ix(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y)&&(n.uniform2i(this.addr,e.x,e.y),t[0]=e.x,t[1]=e.y);else{if(hn(t,e))return;n.uniform2iv(this.addr,e),dn(t,e)}}function kx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z)&&(n.uniform3i(this.addr,e.x,e.y,e.z),t[0]=e.x,t[1]=e.y,t[2]=e.z);else{if(hn(t,e))return;n.uniform3iv(this.addr,e),dn(t,e)}}function Lx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z||t[3]!==e.w)&&(n.uniform4i(this.addr,e.x,e.y,e.z,e.w),t[0]=e.x,t[1]=e.y,t[2]=e.z,t[3]=e.w);else{if(hn(t,e))return;n.uniform4iv(this.addr,e),dn(t,e)}}function Dx(n,e){let t=this.cache;t[0]!==e&&(n.uniform1ui(this.addr,e),t[0]=e)}function Nx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y)&&(n.uniform2ui(this.addr,e.x,e.y),t[0]=e.x,t[1]=e.y);else{if(hn(t,e))return;n.uniform2uiv(this.addr,e),dn(t,e)}}function Ux(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z)&&(n.uniform3ui(this.addr,e.x,e.y,e.z),t[0]=e.x,t[1]=e.y,t[2]=e.z);else{if(hn(t,e))return;n.uniform3uiv(this.addr,e),dn(t,e)}}function Fx(n,e){let t=this.cache;if(e.x!==void 0)(t[0]!==e.x||t[1]!==e.y||t[2]!==e.z||t[3]!==e.w)&&(n.uniform4ui(this.addr,e.x,e.y,e.z,e.w),t[0]=e.x,t[1]=e.y,t[2]=e.z,t[3]=e.w);else{if(hn(t,e))return;n.uniform4uiv(this.addr,e),dn(t,e)}}function Ox(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r);let s;this.type===n.SAMPLER_2D_SHADOW?(cu.compareFunction=t.isReversedDepthBuffer()?_l:yl,s=cu):s=Od,t.setTexture2D(e||s,r)}function Bx(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r),t.setTexture3D(e||Hd,r)}function Hx(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r),t.setTextureCube(e||zd,r)}function zx(n,e,t){let i=this.cache,r=t.allocateTextureUnit();i[0]!==r&&(n.uniform1i(this.addr,r),i[0]=r),t.setTexture2DArray(e||Bd,r)}function Gx(n){switch(n){case 5126:return Sx;case 35664:return wx;case 35665:return Ex;case 35666:return Tx;case 35674:return Ax;case 35675:return Rx;case 35676:return Cx;case 5124:case 35670:return Px;case 35667:case 35671:return Ix;case 35668:case 35672:return kx;case 35669:case 35673:return Lx;case 5125:return Dx;case 36294:return Nx;case 36295:return Ux;case 36296:return Fx;case 35678:case 36198:case 36298:case 36306:case 35682:return Ox;case 35679:case 36299:case 36307:return Bx;case 35680:case 36300:case 36308:case 36293:return Hx;case 36289:case 36303:case 36311:case 36292:return zx}}function Vx(n,e){n.uniform1fv(this.addr,e)}function Wx(n,e){let t=_s(e,this.size,2);n.uniform2fv(this.addr,t)}function Xx(n,e){let t=_s(e,this.size,3);n.uniform3fv(this.addr,t)}function qx(n,e){let t=_s(e,this.size,4);n.uniform4fv(this.addr,t)}function $x(n,e){let t=_s(e,this.size,4);n.uniformMatrix2fv(this.addr,!1,t)}function Yx(n,e){let t=_s(e,this.size,9);n.uniformMatrix3fv(this.addr,!1,t)}function Zx(n,e){let t=_s(e,this.size,16);n.uniformMatrix4fv(this.addr,!1,t)}function Kx(n,e){n.uniform1iv(this.addr,e)}function Jx(n,e){n.uniform2iv(this.addr,e)}function jx(n,e){n.uniform3iv(this.addr,e)}function Qx(n,e){n.uniform4iv(this.addr,e)}function ev(n,e){n.uniform1uiv(this.addr,e)}function tv(n,e){n.uniform2uiv(this.addr,e)}function nv(n,e){n.uniform3uiv(this.addr,e)}function iv(n,e){n.uniform4uiv(this.addr,e)}function rv(n,e,t){let i=this.cache,r=e.length,s=Al(t,r);hn(i,s)||(n.uniform1iv(this.addr,s),dn(i,s));let o;this.type===n.SAMPLER_2D_SHADOW?o=cu:o=Od;for(let a=0;a!==r;++a)t.setTexture2D(e[a]||o,s[a])}function sv(n,e,t){let i=this.cache,r=e.length,s=Al(t,r);hn(i,s)||(n.uniform1iv(this.addr,s),dn(i,s));for(let o=0;o!==r;++o)t.setTexture3D(e[o]||Hd,s[o])}function ov(n,e,t){let i=this.cache,r=e.length,s=Al(t,r);hn(i,s)||(n.uniform1iv(this.addr,s),dn(i,s));for(let o=0;o!==r;++o)t.setTextureCube(e[o]||zd,s[o])}function av(n,e,t){let i=this.cache,r=e.length,s=Al(t,r);hn(i,s)||(n.uniform1iv(this.addr,s),dn(i,s));for(let o=0;o!==r;++o)t.setTexture2DArray(e[o]||Bd,s[o])}function lv(n){switch(n){case 5126:return Vx;case 35664:return Wx;case 35665:return Xx;case 35666:return qx;case 35674:return $x;case 35675:return Yx;case 35676:return Zx;case 5124:case 35670:return Kx;case 35667:case 35671:return Jx;case 35668:case 35672:return jx;case 35669:case 35673:return Qx;case 5125:return ev;case 36294:return tv;case 36295:return nv;case 36296:return iv;case 35678:case 36198:case 36298:case 36306:case 35682:return rv;case 35679:case 36299:case 36307:return sv;case 35680:case 36300:case 36308:case 36293:return ov;case 36289:case 36303:case 36311:case 36292:return av}}var uu=class{constructor(e,t,i){this.id=e,this.addr=i,this.cache=[],this.type=t.type,this.setValue=Gx(t.type)}},hu=class{constructor(e,t,i){this.id=e,this.addr=i,this.cache=[],this.type=t.type,this.size=t.size,this.setValue=lv(t.type)}},du=class{constructor(e){this.id=e,this.seq=[],this.map={}}setValue(e,t,i){let r=this.seq;for(let s=0,o=r.length;s!==o;++s){let a=r[s];a.setValue(e,t[a.id],i)}}},au=/(\w+)(\])?(\[|\.)?/g;function wd(n,e){n.seq.push(e),n.map[e.id]=e}function cv(n,e,t){let i=n.name,r=i.length;for(au.lastIndex=0;;){let s=au.exec(i),o=au.lastIndex,a=s[1],l=s[2]==="]",c=s[3];if(l&&(a=a|0),c===void 0||c==="["&&o+2===r){wd(t,c===void 0?new uu(a,n,e):new hu(a,n,e));break}else{let d=t.map[a];d===void 0&&(d=new du(a),wd(t,d)),t=d}}}var vs=class{constructor(e,t){this.seq=[],this.map={};let i=e.getProgramParameter(t,e.ACTIVE_UNIFORMS);for(let o=0;o<i;++o){let a=e.getActiveUniform(t,o),l=e.getUniformLocation(t,a.name);cv(a,l,this)}let r=[],s=[];for(let o of this.seq)o.type===e.SAMPLER_2D_SHADOW||o.type===e.SAMPLER_CUBE_SHADOW||o.type===e.SAMPLER_2D_ARRAY_SHADOW?r.push(o):s.push(o);r.length>0&&(this.seq=r.concat(s))}setValue(e,t,i,r){let s=this.map[t];s!==void 0&&s.setValue(e,i,r)}setOptional(e,t,i){let r=t[i];r!==void 0&&this.setValue(e,i,r)}static upload(e,t,i,r){for(let s=0,o=t.length;s!==o;++s){let a=t[s],l=i[a.id];l.needsUpdate!==!1&&a.setValue(e,l.value,r)}}static seqWithValue(e,t){let i=[];for(let r=0,s=e.length;r!==s;++r){let o=e[r];o.id in t&&i.push(o)}return i}};function Ed(n,e,t){let i=n.createShader(e);return n.shaderSource(i,t),n.compileShader(i),i}var uv=37297,hv=0;function dv(n,e){let t=n.split(`
`),i=[],r=Math.max(e-6,0),s=Math.min(e+6,t.length);for(let o=r;o<s;o++){let a=o+1;i.push(`${a===e?">":" "} ${a}: ${t[o]}`)}return i.join(`
`)}var Td=new je;function fv(n){ht._getMatrix(Td,ht.workingColorSpace,n);let e=`mat3( ${Td.elements.map(t=>t.toFixed(4))} )`;switch(ht.getTransfer(n)){case Bs:return[e,"LinearTransferOETF"];case St:return[e,"sRGBTransferOETF"];default:return Ye("WebGLProgram: Unsupported color space: ",n),[e,"LinearTransferOETF"]}}function Ad(n,e,t){let i=n.getShaderParameter(e,n.COMPILE_STATUS),s=(n.getShaderInfoLog(e)||"").trim();if(i&&s==="")return"";let o=/ERROR: 0:(\d+)/.exec(s);if(o){let a=parseInt(o[1]);return t.toUpperCase()+`

`+s+`

`+dv(n.getShaderSource(e),a)}else return s}function pv(n,e){let t=fv(e);return[`vec4 ${n}( vec4 value ) {`,`	return ${t[1]}( vec4( value.rgb * ${t[0]}, value.a ) );`,"}"].join(`
`)}var mv={[Nc]:"Linear",[Uc]:"Reinhard",[Fc]:"Cineon",[Oc]:"ACESFilmic",[Hc]:"AgX",[zc]:"Neutral",[Bc]:"Custom"};function gv(n,e){let t=mv[e];return t===void 0?(Ye("WebGLProgram: Unsupported toneMapping:",e),"vec3 "+n+"( vec3 color ) { return LinearToneMapping( color ); }"):"vec3 "+n+"( vec3 color ) { return "+t+"ToneMapping( color ); }"}var Sl=new G;function xv(){ht.getLuminanceCoefficients(Sl);let n=Sl.x.toFixed(4),e=Sl.y.toFixed(4),t=Sl.z.toFixed(4);return["float luminance( const in vec3 rgb ) {",`	const vec3 weights = vec3( ${n}, ${e}, ${t} );`,"	return dot( weights, rgb );","}"].join(`
`)}function vv(n){return[n.extensionClipCullDistance?"#extension GL_ANGLE_clip_cull_distance : require":"",n.extensionMultiDraw?"#extension GL_ANGLE_multi_draw : require":""].filter(go).join(`
`)}function yv(n){let e=[];for(let t in n){let i=n[t];i!==!1&&e.push("#define "+t+" "+i)}return e.join(`
`)}function _v(n,e){let t={},i=n.getProgramParameter(e,n.ACTIVE_ATTRIBUTES);for(let r=0;r<i;r++){let s=n.getActiveAttrib(e,r),o=s.name,a=1;s.type===n.FLOAT_MAT2&&(a=2),s.type===n.FLOAT_MAT3&&(a=3),s.type===n.FLOAT_MAT4&&(a=4),t[o]={type:s.type,location:n.getAttribLocation(e,o),locationSize:a}}return t}function go(n){return n!==""}function Rd(n,e){let t=e.numSpotLightShadows+e.numSpotLightMaps-e.numSpotLightShadowsWithMaps;return n.replace(/NUM_SUN_LIGHTS/g,e.numSunLights).replace(/NUM_DIR_LIGHTS/g,e.numDirLights).replace(/NUM_SPOT_LIGHTS/g,e.numSpotLights).replace(/NUM_SPOT_LIGHT_MAPS/g,e.numSpotLightMaps).replace(/NUM_SPOT_LIGHT_COORDS/g,t).replace(/NUM_RECT_AREA_LIGHTS/g,e.numRectAreaLights).replace(/NUM_POINT_LIGHTS/g,e.numPointLights).replace(/NUM_HEMI_LIGHTS/g,e.numHemiLights).replace(/NUM_SUN_LIGHT_SHADOWS/g,e.numSunLightShadows).replace(/NUM_DIR_LIGHT_SHADOWS/g,e.numDirLightShadows).replace(/NUM_SPOT_LIGHT_SHADOWS_WITH_MAPS/g,e.numSpotLightShadowsWithMaps).replace(/NUM_SPOT_LIGHT_SHADOWS/g,e.numSpotLightShadows).replace(/NUM_POINT_LIGHT_SHADOWS/g,e.numPointLightShadows)}function Cd(n,e){return n.replace(/NUM_CLIPPING_PLANES/g,e.numClippingPlanes).replace(/UNION_CLIPPING_PLANES/g,e.numClippingPlanes-e.numClipIntersection)}var bv=/^[ \t]*#include +<([\w\d./]+)>/gm;function fu(n){return n.replace(bv,Sv)}var Mv=new Map;function Sv(n,e){let t=rt[e];if(t===void 0){let i=Mv.get(e);if(i!==void 0)t=rt[i],Ye('WebGLRenderer: Shader chunk "%s" has been deprecated. Use "%s" instead.',e,i);else throw new Error("THREE.WebGLProgram: Can not resolve #include <"+e+">")}return fu(t)}var wv=/#pragma unroll_loop_start\s+for\s*\(\s*int\s+i\s*=\s*(\d+)\s*;\s*i\s*<\s*(\d+)\s*;\s*i\s*\+\+\s*\)\s*{([\s\S]+?)}\s+#pragma unroll_loop_end/g;function Pd(n){return n.replace(wv,Ev)}function Ev(n,e,t,i){let r="";for(let s=parseInt(e);s<parseInt(t);s++)r+=i.replace(/\[\s*i\s*\]/g,"[ "+s+" ]").replace(/UNROLLED_LOOP_INDEX/g,s);return r}function Id(n){let e=`precision ${n.precision} float;
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
#define LOW_PRECISION`),e}var Tv={[Mr]:"SHADOWMAP_TYPE_PCF",[hs]:"SHADOWMAP_TYPE_VSM"};function Av(n){return Tv[n.shadowMapType]||"SHADOWMAP_TYPE_BASIC"}var Rv={[nr]:"ENVMAP_TYPE_CUBE",[wr]:"ENVMAP_TYPE_CUBE",[so]:"ENVMAP_TYPE_CUBE_UV"};function Cv(n){return n.envMap===!1?"ENVMAP_TYPE_CUBE":Rv[n.envMapMode]||"ENVMAP_TYPE_CUBE"}var Pv={[wr]:"ENVMAP_MODE_REFRACTION"};function Iv(n){return n.envMap===!1?"ENVMAP_MODE_REFLECTION":Pv[n.envMapMode]||"ENVMAP_MODE_REFLECTION"}var kv={[Dc]:"ENVMAP_BLENDING_MULTIPLY",[Zh]:"ENVMAP_BLENDING_MIX",[Kh]:"ENVMAP_BLENDING_ADD"};function Lv(n){return n.envMap===!1?"ENVMAP_BLENDING_NONE":kv[n.combine]||"ENVMAP_BLENDING_NONE"}function Dv(n){let e=n.envMapCubeUVHeight;if(e===null)return null;let t=Math.log2(e)-2,i=1/e;return{texelWidth:1/(3*Math.max(Math.pow(2,t),112)),texelHeight:i,maxMip:t}}function Nv(n,e,t,i){let r=n.getContext(),s=t.defines,o=t.vertexShader,a=t.fragmentShader,l=Av(t),c=Cv(t),h=Iv(t),d=Lv(t),u=Dv(t),p=vv(t),x=yv(s),v=r.createProgram(),m,f,b=t.glslVersion?"#version "+t.glslVersion+`
`:"";t.isRawShaderMaterial?(m=["#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,x].filter(go).join(`
`),m.length>0&&(m+=`
`),f=["#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,x].filter(go).join(`
`),f.length>0&&(f+=`
`)):(m=[Id(t),"#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,x,t.extensionClipCullDistance?"#define USE_CLIP_DISTANCE":"",t.batching?"#define USE_BATCHING":"",t.batchingColor?"#define USE_BATCHING_COLOR":"",t.instancing?"#define USE_INSTANCING":"",t.instancingColor?"#define USE_INSTANCING_COLOR":"",t.instancingMorph?"#define USE_INSTANCING_MORPH":"",t.useFog&&t.fog?"#define USE_FOG":"",t.useFog&&t.fogExp2?"#define FOG_EXP2":"",t.map?"#define USE_MAP":"",t.envMap?"#define USE_ENVMAP":"",t.envMap?"#define "+h:"",t.lightMap?"#define USE_LIGHTMAP":"",t.aoMap?"#define USE_AOMAP":"",t.bumpMap?"#define USE_BUMPMAP":"",t.normalMap?"#define USE_NORMALMAP":"",t.normalMapObjectSpace?"#define USE_NORMALMAP_OBJECTSPACE":"",t.normalMapTangentSpace?"#define USE_NORMALMAP_TANGENTSPACE":"",t.displacementMap?"#define USE_DISPLACEMENTMAP":"",t.emissiveMap?"#define USE_EMISSIVEMAP":"",t.anisotropy?"#define USE_ANISOTROPY":"",t.anisotropyMap?"#define USE_ANISOTROPYMAP":"",t.clearcoatMap?"#define USE_CLEARCOATMAP":"",t.clearcoatRoughnessMap?"#define USE_CLEARCOAT_ROUGHNESSMAP":"",t.clearcoatNormalMap?"#define USE_CLEARCOAT_NORMALMAP":"",t.iridescenceMap?"#define USE_IRIDESCENCEMAP":"",t.iridescenceThicknessMap?"#define USE_IRIDESCENCE_THICKNESSMAP":"",t.specularMap?"#define USE_SPECULARMAP":"",t.specularColorMap?"#define USE_SPECULAR_COLORMAP":"",t.specularIntensityMap?"#define USE_SPECULAR_INTENSITYMAP":"",t.roughnessMap?"#define USE_ROUGHNESSMAP":"",t.metalnessMap?"#define USE_METALNESSMAP":"",t.alphaMap?"#define USE_ALPHAMAP":"",t.alphaHash?"#define USE_ALPHAHASH":"",t.transmission?"#define USE_TRANSMISSION":"",t.transmissionMap?"#define USE_TRANSMISSIONMAP":"",t.thicknessMap?"#define USE_THICKNESSMAP":"",t.sheenColorMap?"#define USE_SHEEN_COLORMAP":"",t.sheenRoughnessMap?"#define USE_SHEEN_ROUGHNESSMAP":"",t.mapUv?"#define MAP_UV "+t.mapUv:"",t.alphaMapUv?"#define ALPHAMAP_UV "+t.alphaMapUv:"",t.lightMapUv?"#define LIGHTMAP_UV "+t.lightMapUv:"",t.aoMapUv?"#define AOMAP_UV "+t.aoMapUv:"",t.emissiveMapUv?"#define EMISSIVEMAP_UV "+t.emissiveMapUv:"",t.bumpMapUv?"#define BUMPMAP_UV "+t.bumpMapUv:"",t.normalMapUv?"#define NORMALMAP_UV "+t.normalMapUv:"",t.displacementMapUv?"#define DISPLACEMENTMAP_UV "+t.displacementMapUv:"",t.metalnessMapUv?"#define METALNESSMAP_UV "+t.metalnessMapUv:"",t.roughnessMapUv?"#define ROUGHNESSMAP_UV "+t.roughnessMapUv:"",t.anisotropyMapUv?"#define ANISOTROPYMAP_UV "+t.anisotropyMapUv:"",t.clearcoatMapUv?"#define CLEARCOATMAP_UV "+t.clearcoatMapUv:"",t.clearcoatNormalMapUv?"#define CLEARCOAT_NORMALMAP_UV "+t.clearcoatNormalMapUv:"",t.clearcoatRoughnessMapUv?"#define CLEARCOAT_ROUGHNESSMAP_UV "+t.clearcoatRoughnessMapUv:"",t.iridescenceMapUv?"#define IRIDESCENCEMAP_UV "+t.iridescenceMapUv:"",t.iridescenceThicknessMapUv?"#define IRIDESCENCE_THICKNESSMAP_UV "+t.iridescenceThicknessMapUv:"",t.sheenColorMapUv?"#define SHEEN_COLORMAP_UV "+t.sheenColorMapUv:"",t.sheenRoughnessMapUv?"#define SHEEN_ROUGHNESSMAP_UV "+t.sheenRoughnessMapUv:"",t.specularMapUv?"#define SPECULARMAP_UV "+t.specularMapUv:"",t.specularColorMapUv?"#define SPECULAR_COLORMAP_UV "+t.specularColorMapUv:"",t.specularIntensityMapUv?"#define SPECULAR_INTENSITYMAP_UV "+t.specularIntensityMapUv:"",t.transmissionMapUv?"#define TRANSMISSIONMAP_UV "+t.transmissionMapUv:"",t.thicknessMapUv?"#define THICKNESSMAP_UV "+t.thicknessMapUv:"",t.vertexTangents&&t.flatShading===!1?"#define USE_TANGENT":"",t.vertexNormals?"#define HAS_NORMAL":"",t.vertexColors?"#define USE_COLOR":"",t.vertexAlphas?"#define USE_COLOR_ALPHA":"",t.vertexUv1s?"#define USE_UV1":"",t.vertexUv2s?"#define USE_UV2":"",t.vertexUv3s?"#define USE_UV3":"",t.pointsUvs?"#define USE_POINTS_UV":"",t.flatShading?"#define FLAT_SHADED":"",t.skinning?"#define USE_SKINNING":"",t.morphTargets?"#define USE_MORPHTARGETS":"",t.morphNormals&&t.flatShading===!1?"#define USE_MORPHNORMALS":"",t.morphColors?"#define USE_MORPHCOLORS":"",t.morphTargetsCount>0?"#define MORPHTARGETS_TEXTURE_STRIDE "+t.morphTextureStride:"",t.morphTargetsCount>0?"#define MORPHTARGETS_COUNT "+t.morphTargetsCount:"",t.doubleSided?"#define DOUBLE_SIDED":"",t.flipSided?"#define FLIP_SIDED":"",t.shadowMapEnabled?"#define USE_SHADOWMAP":"",t.shadowMapEnabled?"#define "+l:"",t.sizeAttenuation?"#define USE_SIZEATTENUATION":"",t.numLightProbes>0?"#define USE_LIGHT_PROBES":"",t.logarithmicDepthBuffer?"#define USE_LOGARITHMIC_DEPTH_BUFFER":"",t.reversedDepthBuffer?"#define USE_REVERSED_DEPTH_BUFFER":"","uniform mat4 modelMatrix;","uniform mat4 modelViewMatrix;","uniform mat4 projectionMatrix;","uniform mat4 viewMatrix;","uniform mat3 normalMatrix;","uniform vec3 cameraPosition;","uniform bool isOrthographic;","#ifdef USE_INSTANCING","	attribute mat4 instanceMatrix;","#endif","#ifdef USE_INSTANCING_COLOR","	attribute vec3 instanceColor;","#endif","#ifdef USE_INSTANCING_MORPH","	uniform sampler2D morphTexture;","#endif","attribute vec3 position;","attribute vec3 normal;","attribute vec2 uv;","#ifdef USE_UV1","	attribute vec2 uv1;","#endif","#ifdef USE_UV2","	attribute vec2 uv2;","#endif","#ifdef USE_UV3","	attribute vec2 uv3;","#endif","#ifdef USE_TANGENT","	attribute vec4 tangent;","#endif","#if defined( USE_COLOR_ALPHA )","	attribute vec4 color;","#elif defined( USE_COLOR )","	attribute vec3 color;","#endif","#ifdef USE_SKINNING","	attribute vec4 skinIndex;","	attribute vec4 skinWeight;","#endif",`
`].filter(go).join(`
`),f=[Id(t),"#define SHADER_TYPE "+t.shaderType,"#define SHADER_NAME "+t.shaderName,x,t.useFog&&t.fog?"#define USE_FOG":"",t.useFog&&t.fogExp2?"#define FOG_EXP2":"",t.alphaToCoverage?"#define ALPHA_TO_COVERAGE":"",t.map?"#define USE_MAP":"",t.matcap?"#define USE_MATCAP":"",t.envMap?"#define USE_ENVMAP":"",t.envMap?"#define "+c:"",t.envMap?"#define "+h:"",t.envMap?"#define "+d:"",u?"#define CUBEUV_TEXEL_WIDTH "+u.texelWidth:"",u?"#define CUBEUV_TEXEL_HEIGHT "+u.texelHeight:"",u?"#define CUBEUV_MAX_MIP "+u.maxMip+".0":"",t.lightMap?"#define USE_LIGHTMAP":"",t.aoMap?"#define USE_AOMAP":"",t.bumpMap?"#define USE_BUMPMAP":"",t.normalMap?"#define USE_NORMALMAP":"",t.normalMapObjectSpace?"#define USE_NORMALMAP_OBJECTSPACE":"",t.normalMapTangentSpace?"#define USE_NORMALMAP_TANGENTSPACE":"",t.packedNormalMap?"#define USE_PACKED_NORMALMAP":"",t.emissiveMap?"#define USE_EMISSIVEMAP":"",t.anisotropy?"#define USE_ANISOTROPY":"",t.anisotropyMap?"#define USE_ANISOTROPYMAP":"",t.clearcoat?"#define USE_CLEARCOAT":"",t.clearcoatMap?"#define USE_CLEARCOATMAP":"",t.clearcoatRoughnessMap?"#define USE_CLEARCOAT_ROUGHNESSMAP":"",t.clearcoatNormalMap?"#define USE_CLEARCOAT_NORMALMAP":"",t.dispersion?"#define USE_DISPERSION":"",t.retroreflection?"#define USE_RETROREFLECTION":"",t.iridescence?"#define USE_IRIDESCENCE":"",t.iridescenceMap?"#define USE_IRIDESCENCEMAP":"",t.iridescenceThicknessMap?"#define USE_IRIDESCENCE_THICKNESSMAP":"",t.specularMap?"#define USE_SPECULARMAP":"",t.specularColorMap?"#define USE_SPECULAR_COLORMAP":"",t.specularIntensityMap?"#define USE_SPECULAR_INTENSITYMAP":"",t.roughnessMap?"#define USE_ROUGHNESSMAP":"",t.metalnessMap?"#define USE_METALNESSMAP":"",t.alphaMap?"#define USE_ALPHAMAP":"",t.alphaTest?"#define USE_ALPHATEST":"",t.alphaHash?"#define USE_ALPHAHASH":"",t.sheen?"#define USE_SHEEN":"",t.sheenColorMap?"#define USE_SHEEN_COLORMAP":"",t.sheenRoughnessMap?"#define USE_SHEEN_ROUGHNESSMAP":"",t.transmission?"#define USE_TRANSMISSION":"",t.transmissionMap?"#define USE_TRANSMISSIONMAP":"",t.thicknessMap?"#define USE_THICKNESSMAP":"",t.vertexTangents&&t.flatShading===!1?"#define USE_TANGENT":"",t.vertexColors||t.instancingColor?"#define USE_COLOR":"",t.vertexAlphas||t.batchingColor?"#define USE_COLOR_ALPHA":"",t.vertexUv1s?"#define USE_UV1":"",t.vertexUv2s?"#define USE_UV2":"",t.vertexUv3s?"#define USE_UV3":"",t.pointsUvs?"#define USE_POINTS_UV":"",t.gradientMap?"#define USE_GRADIENTMAP":"",t.flatShading?"#define FLAT_SHADED":"",t.doubleSided?"#define DOUBLE_SIDED":"",t.flipSided?"#define FLIP_SIDED":"",t.shadowMapEnabled?"#define USE_SHADOWMAP":"",t.shadowMapEnabled?"#define "+l:"",t.premultipliedAlpha?"#define PREMULTIPLIED_ALPHA":"",t.numLightProbes>0?"#define USE_LIGHT_PROBES":"",t.numLightProbeGrids>0?"#define USE_LIGHT_PROBES_GRID":"",t.decodeVideoTexture?"#define DECODE_VIDEO_TEXTURE":"",t.decodeVideoTextureEmissive?"#define DECODE_VIDEO_TEXTURE_EMISSIVE":"",t.logarithmicDepthBuffer?"#define USE_LOGARITHMIC_DEPTH_BUFFER":"",t.reversedDepthBuffer?"#define USE_REVERSED_DEPTH_BUFFER":"","uniform mat4 viewMatrix;","uniform vec3 cameraPosition;","uniform bool isOrthographic;",t.toneMapping!==ci?"#define TONE_MAPPING":"",t.toneMapping!==ci?rt.tonemapping_pars_fragment:"",t.toneMapping!==ci?gv("toneMapping",t.toneMapping):"",t.dithering?"#define DITHERING":"",t.opaque?"#define OPAQUE":"",rt.colorspace_pars_fragment,pv("linearToOutputTexel",t.outputColorSpace),xv(),t.useDepthPacking?"#define DEPTH_PACKING "+t.depthPacking:"",`
`].filter(go).join(`
`)),o=fu(o),o=Rd(o,t),o=Cd(o,t),a=fu(a),a=Rd(a,t),a=Cd(a,t),o=Pd(o),a=Pd(a),t.isRawShaderMaterial!==!0&&(b=`#version 300 es
`,m=[p,"#define attribute in","#define varying out","#define texture2D texture"].join(`
`)+`
`+m,f=["#define varying in",t.glslVersion===Zc?"":"layout(location = 0) out highp vec4 pc_fragColor;",t.glslVersion===Zc?"":"#define gl_FragColor pc_fragColor","#define gl_FragDepthEXT gl_FragDepth","#define texture2D texture","#define textureCube texture","#define texture2DProj textureProj","#define texture2DLodEXT textureLod","#define texture2DProjLodEXT textureProjLod","#define textureCubeLodEXT textureLod","#define texture2DGradEXT textureGrad","#define texture2DProjGradEXT textureProjGrad","#define textureCubeGradEXT textureGrad"].join(`
`)+`
`+f);let P=b+m+o,w=b+f+a,T=Ed(r,r.VERTEX_SHADER,P),E=Ed(r,r.FRAGMENT_SHADER,w);r.attachShader(v,T),r.attachShader(v,E),t.index0AttributeName!==void 0?r.bindAttribLocation(v,0,t.index0AttributeName):t.hasPositionAttribute===!0&&r.bindAttribLocation(v,0,"position"),r.linkProgram(v);function D(A){if(n.debug.checkShaderErrors){let I=r.getProgramInfoLog(v)||"",k=r.getShaderInfoLog(T)||"",N=r.getShaderInfoLog(E)||"",U=I.trim(),z=k.trim(),Y=N.trim(),ee=!0,J=!0;if(r.getProgramParameter(v,r.LINK_STATUS)===!1)if(ee=!1,typeof n.debug.onShaderError=="function")n.debug.onShaderError(r,v,T,E);else{let ie=Ad(r,T,"vertex"),ce=Ad(r,E,"fragment");Ke("WebGLProgram: Shader Error "+r.getError()+" - VALIDATE_STATUS "+r.getProgramParameter(v,r.VALIDATE_STATUS)+`

Material Name: `+A.name+`
Material Type: `+A.type+`

Program Info Log: `+U+`
`+ie+`
`+ce)}else U!==""?Ye("WebGLProgram: Program Info Log:",U):(z===""||Y==="")&&(J=!1);J&&(A.diagnostics={runnable:ee,programLog:U,vertexShader:{log:z,prefix:m},fragmentShader:{log:Y,prefix:f}})}r.deleteShader(T),r.deleteShader(E),g=new vs(r,v),S=_v(r,v)}let g;this.getUniforms=function(){return g===void 0&&D(this),g};let S;this.getAttributes=function(){return S===void 0&&D(this),S};let _=t.rendererExtensionParallelShaderCompile===!1;return this.isReady=function(){return _===!1&&(_=r.getProgramParameter(v,uv)),_},this.destroy=function(){i.releaseStatesOfProgram(this),r.deleteProgram(v),this.program=void 0},this.type=t.shaderType,this.name=t.shaderName,this.id=hv++,this.cacheKey=e,this.usedTimes=1,this.program=v,this.vertexShader=T,this.fragmentShader=E,this}var Uv=0,pu=class{constructor(){this.shaderCache=new Map,this.materialCache=new Map}update(e,t,i){let r=this._getShaderCacheForMaterial(e);return r.has(t)===!1&&(r.add(t),t.usedTimes++),r.has(i)===!1&&(r.add(i),i.usedTimes++),this}remove(e){let t=this.materialCache.get(e);for(let i of t)i.usedTimes--,i.usedTimes===0&&this.shaderCache.delete(i.code);return this.materialCache.delete(e),this}getVertexShaderStage(e){return this._getShaderStage(e.vertexShader)}getFragmentShaderStage(e){return this._getShaderStage(e.fragmentShader)}dispose(){this.shaderCache.clear(),this.materialCache.clear()}_getShaderCacheForMaterial(e){let t=this.materialCache,i=t.get(e);return i===void 0&&(i=new Set,t.set(e,i)),i}_getShaderStage(e){let t=this.shaderCache,i=t.get(e);return i===void 0&&(i=new mu(e),t.set(e,i)),i}},mu=class{constructor(e){this.id=Uv++,this.code=e,this.usedTimes=0}};function Fv(n){return n===sr||n===ho||n===fo}function Ov(n,e,t,i,r,s){let o=new rs,a=new pu,l=new Set,c=[],h=new Map,d=i.logarithmicDepthBuffer,u=i.precision,p={MeshDepthMaterial:"depth",MeshDistanceMaterial:"distance",MeshNormalMaterial:"normal",MeshBasicMaterial:"basic",MeshLambertMaterial:"lambert",MeshPhongMaterial:"phong",MeshToonMaterial:"toon",MeshStandardMaterial:"physical",MeshPhysicalMaterial:"physical",MeshMatcapMaterial:"matcap",LineBasicMaterial:"basic",LineDashedMaterial:"dashed",PointsMaterial:"points",ShadowMaterial:"shadow",SpriteMaterial:"sprite"};function x(g){return l.add(g),g===0?"uv":`uv${g}`}function v(g,S,_,A,I,k){let N=A.fog,U=I.geometry,z=g.isMeshStandardMaterial||g.isMeshLambertMaterial||g.isMeshPhongMaterial?A.environment:null,Y=g.isMeshStandardMaterial||g.isMeshLambertMaterial&&!g.envMap||g.isMeshPhongMaterial&&!g.envMap,ee=e.get(g.envMap||z,Y),J=ee&&ee.mapping===so?ee.image.height:null,ie=p[g.type];g.precision!==null&&(u=i.getMaxPrecision(g.precision),u!==g.precision&&Ye("WebGLProgram.getParameters:",g.precision,"not supported, using",u,"instead."));let ce=U.morphAttributes.position||U.morphAttributes.normal||U.morphAttributes.color,Se=ce!==void 0?ce.length:0,fe=0;U.morphAttributes.position!==void 0&&(fe=1),U.morphAttributes.normal!==void 0&&(fe=2),U.morphAttributes.color!==void 0&&(fe=3);let Ie,We,He,oe;if(ie){let vt=Mi[ie];Ie=vt.vertexShader,We=vt.fragmentShader}else{Ie=g.vertexShader,We=g.fragmentShader;let vt=a.getVertexShaderStage(g),ot=a.getFragmentShaderStage(g);a.update(g,vt,ot),He=vt.id,oe=ot.id}let le=n.getRenderTarget(),be=n.state.buffers.depth.getReversed(),Ve=I.isInstancedMesh===!0,Me=I.isBatchedMesh===!0,Ge=!!g.map,Bt=!!g.matcap,Je=!!ee,st=!!g.aoMap,mt=!!g.lightMap,tt=!!g.bumpMap&&g.wireframe===!1,Et=!!g.normalMap,Ht=!!g.displacementMap,$t=!!g.emissiveMap,It=!!g.metalnessMap,zt=!!g.roughnessMap,W=g.anisotropy>0,nn=g.clearcoat>0,ut=g.dispersion>0,F=g.retroreflectivity>0,M=g.iridescence>0,Z=g.sheen>0,te=g.transmission>0,ae=W&&!!g.anisotropyMap,ve=nn&&!!g.clearcoatMap,_e=nn&&!!g.clearcoatNormalMap,C=nn&&!!g.clearcoatRoughnessMap,L=M&&!!g.iridescenceMap,O=M&&!!g.iridescenceThicknessMap,H=Z&&!!g.sheenColorMap,q=Z&&!!g.sheenRoughnessMap,K=!!g.specularMap,re=!!g.specularColorMap,pe=!!g.specularIntensityMap,me=te&&!!g.transmissionMap,B=te&&!!g.thicknessMap,ue=!!g.gradientMap,se=!!g.alphaMap,ge=g.alphaTest>0,ye=!!g.alphaHash,de=!!g.extensions,Pe=ci;g.toneMapped&&(le===null||le.isXRRenderTarget===!0)&&(Pe=n.toneMapping);let Le={shaderID:ie,shaderType:g.type,shaderName:g.name,vertexShader:Ie,fragmentShader:We,defines:g.defines,customVertexShaderID:He,customFragmentShaderID:oe,isRawShaderMaterial:g.isRawShaderMaterial===!0,glslVersion:g.glslVersion,precision:u,batching:Me,batchingColor:Me&&I._colorsTexture!==null,instancing:Ve,instancingColor:Ve&&I.instanceColor!==null,instancingMorph:Ve&&I.morphTexture!==null,outputColorSpace:le===null?n.outputColorSpace:le.isXRRenderTarget===!0?le.texture.colorSpace:ht.workingColorSpace,alphaToCoverage:!!g.alphaToCoverage,map:Ge,matcap:Bt,envMap:Je,envMapMode:Je&&ee.mapping,envMapCubeUVHeight:J,aoMap:st,lightMap:mt,bumpMap:tt,normalMap:Et,displacementMap:Ht,emissiveMap:$t,normalMapObjectSpace:Et&&g.normalMapType===Qh,normalMapTangentSpace:Et&&g.normalMapType===vl,packedNormalMap:Et&&g.normalMapType===vl&&Fv(g.normalMap.format),metalnessMap:It,roughnessMap:zt,anisotropy:W,anisotropyMap:ae,clearcoat:nn,clearcoatMap:ve,clearcoatNormalMap:_e,clearcoatRoughnessMap:C,dispersion:ut,retroreflection:F,iridescence:M,iridescenceMap:L,iridescenceThicknessMap:O,sheen:Z,sheenColorMap:H,sheenRoughnessMap:q,specularMap:K,specularColorMap:re,specularIntensityMap:pe,transmission:te,transmissionMap:me,thicknessMap:B,gradientMap:ue,opaque:g.transparent===!1&&g.blending===ds&&g.alphaToCoverage===!1,alphaMap:se,alphaTest:ge,alphaHash:ye,combine:g.combine,mapUv:Ge&&x(g.map.channel),aoMapUv:st&&x(g.aoMap.channel),lightMapUv:mt&&x(g.lightMap.channel),bumpMapUv:tt&&x(g.bumpMap.channel),normalMapUv:Et&&x(g.normalMap.channel),displacementMapUv:Ht&&x(g.displacementMap.channel),emissiveMapUv:$t&&x(g.emissiveMap.channel),metalnessMapUv:It&&x(g.metalnessMap.channel),roughnessMapUv:zt&&x(g.roughnessMap.channel),anisotropyMapUv:ae&&x(g.anisotropyMap.channel),clearcoatMapUv:ve&&x(g.clearcoatMap.channel),clearcoatNormalMapUv:_e&&x(g.clearcoatNormalMap.channel),clearcoatRoughnessMapUv:C&&x(g.clearcoatRoughnessMap.channel),iridescenceMapUv:L&&x(g.iridescenceMap.channel),iridescenceThicknessMapUv:O&&x(g.iridescenceThicknessMap.channel),sheenColorMapUv:H&&x(g.sheenColorMap.channel),sheenRoughnessMapUv:q&&x(g.sheenRoughnessMap.channel),specularMapUv:K&&x(g.specularMap.channel),specularColorMapUv:re&&x(g.specularColorMap.channel),specularIntensityMapUv:pe&&x(g.specularIntensityMap.channel),transmissionMapUv:me&&x(g.transmissionMap.channel),thicknessMapUv:B&&x(g.thicknessMap.channel),alphaMapUv:se&&x(g.alphaMap.channel),vertexTangents:!!U.attributes.tangent&&(Et||W),vertexNormals:!!U.attributes.normal,vertexColors:g.vertexColors,vertexAlphas:g.vertexColors===!0&&!!U.attributes.color&&U.attributes.color.itemSize===4,pointsUvs:I.isPoints===!0&&!!U.attributes.uv&&(Ge||se),fog:!!N,useFog:g.fog===!0,fogExp2:!!N&&N.isFogExp2,flatShading:g.wireframe===!1&&(g.flatShading===!0||U.attributes.normal===void 0&&Et===!1&&(g.isMeshLambertMaterial||g.isMeshPhongMaterial||g.isMeshStandardMaterial||g.isMeshPhysicalMaterial)),sizeAttenuation:g.sizeAttenuation===!0,logarithmicDepthBuffer:d,reversedDepthBuffer:be,skinning:I.isSkinnedMesh===!0,hasPositionAttribute:U.attributes.position!==void 0,morphTargets:U.morphAttributes.position!==void 0,morphNormals:U.morphAttributes.normal!==void 0,morphColors:U.morphAttributes.color!==void 0,morphTargetsCount:Se,morphTextureStride:fe,numSunLights:S.sun.length,numDirLights:S.directional.length,numPointLights:S.point.length,numSpotLights:S.spot.length,numSpotLightMaps:S.spotLightMap.length,numRectAreaLights:S.rectArea.length,numHemiLights:S.hemi.length,numSunLightShadows:S.sunShadowMap.length,numDirLightShadows:S.directionalShadowMap.length,numPointLightShadows:S.pointShadowMap.length,numSpotLightShadows:S.spotShadowMap.length,numSpotLightShadowsWithMaps:S.numSpotLightShadowsWithMaps,numLightProbes:S.numLightProbes,numLightProbeGrids:k.length,numClippingPlanes:s.numPlanes,numClipIntersection:s.numIntersection,dithering:g.dithering,shadowMapEnabled:n.shadowMap.enabled&&_.length>0,shadowMapType:n.shadowMap.type,toneMapping:Pe,decodeVideoTexture:Ge&&g.map.isVideoTexture===!0&&ht.getTransfer(g.map.colorSpace)===St,decodeVideoTextureEmissive:$t&&g.emissiveMap.isVideoTexture===!0&&ht.getTransfer(g.emissiveMap.colorSpace)===St,premultipliedAlpha:g.premultipliedAlpha,doubleSided:g.side===Fn,flipSided:g.side===mn,useDepthPacking:g.depthPacking>=0,depthPacking:g.depthPacking||0,index0AttributeName:g.index0AttributeName,extensionClipCullDistance:de&&g.extensions.clipCullDistance===!0&&t.has("WEBGL_clip_cull_distance"),extensionMultiDraw:(de&&g.extensions.multiDraw===!0||Me)&&t.has("WEBGL_multi_draw"),rendererExtensionParallelShaderCompile:t.has("KHR_parallel_shader_compile"),customProgramCacheKey:g.customProgramCacheKey()};return Le.vertexUv1s=l.has(1),Le.vertexUv2s=l.has(2),Le.vertexUv3s=l.has(3),l.clear(),Le}function m(g){let S=[];if(g.shaderID?S.push(g.shaderID):(S.push(g.customVertexShaderID),S.push(g.customFragmentShaderID)),g.defines!==void 0)for(let _ in g.defines)S.push(_),S.push(g.defines[_]);return g.isRawShaderMaterial===!1&&(f(S,g),b(S,g),S.push(n.outputColorSpace)),S.push(g.customProgramCacheKey),S.join()}function f(g,S){g.push(S.precision),g.push(S.outputColorSpace),g.push(S.envMapMode),g.push(S.envMapCubeUVHeight),g.push(S.mapUv),g.push(S.alphaMapUv),g.push(S.lightMapUv),g.push(S.aoMapUv),g.push(S.bumpMapUv),g.push(S.normalMapUv),g.push(S.displacementMapUv),g.push(S.emissiveMapUv),g.push(S.metalnessMapUv),g.push(S.roughnessMapUv),g.push(S.anisotropyMapUv),g.push(S.clearcoatMapUv),g.push(S.clearcoatNormalMapUv),g.push(S.clearcoatRoughnessMapUv),g.push(S.iridescenceMapUv),g.push(S.iridescenceThicknessMapUv),g.push(S.sheenColorMapUv),g.push(S.sheenRoughnessMapUv),g.push(S.specularMapUv),g.push(S.specularColorMapUv),g.push(S.specularIntensityMapUv),g.push(S.transmissionMapUv),g.push(S.thicknessMapUv),g.push(S.combine),g.push(S.fogExp2),g.push(S.sizeAttenuation),g.push(S.morphTargetsCount),g.push(S.morphAttributeCount),g.push(S.numSunLights),g.push(S.numDirLights),g.push(S.numPointLights),g.push(S.numSpotLights),g.push(S.numSpotLightMaps),g.push(S.numHemiLights),g.push(S.numRectAreaLights),g.push(S.numSunLightShadows),g.push(S.numDirLightShadows),g.push(S.numPointLightShadows),g.push(S.numSpotLightShadows),g.push(S.numSpotLightShadowsWithMaps),g.push(S.numLightProbes),g.push(S.shadowMapType),g.push(S.toneMapping),g.push(S.numClippingPlanes),g.push(S.numClipIntersection),g.push(S.depthPacking)}function b(g,S){o.disableAll(),S.instancing&&o.enable(0),S.instancingColor&&o.enable(1),S.instancingMorph&&o.enable(2),S.matcap&&o.enable(3),S.envMap&&o.enable(4),S.normalMapObjectSpace&&o.enable(5),S.normalMapTangentSpace&&o.enable(6),S.clearcoat&&o.enable(7),S.iridescence&&o.enable(8),S.alphaTest&&o.enable(9),S.vertexColors&&o.enable(10),S.vertexAlphas&&o.enable(11),S.vertexUv1s&&o.enable(12),S.vertexUv2s&&o.enable(13),S.vertexUv3s&&o.enable(14),S.vertexTangents&&o.enable(15),S.anisotropy&&o.enable(16),S.alphaHash&&o.enable(17),S.batching&&o.enable(18),S.dispersion&&o.enable(19),S.retroreflection&&o.enable(24),S.batchingColor&&o.enable(20),S.gradientMap&&o.enable(21),S.packedNormalMap&&o.enable(22),S.vertexNormals&&o.enable(23),g.push(o.mask),o.disableAll(),S.fog&&o.enable(0),S.useFog&&o.enable(1),S.flatShading&&o.enable(2),S.logarithmicDepthBuffer&&o.enable(3),S.reversedDepthBuffer&&o.enable(4),S.skinning&&o.enable(5),S.morphTargets&&o.enable(6),S.morphNormals&&o.enable(7),S.morphColors&&o.enable(8),S.premultipliedAlpha&&o.enable(9),S.shadowMapEnabled&&o.enable(10),S.doubleSided&&o.enable(11),S.flipSided&&o.enable(12),S.useDepthPacking&&o.enable(13),S.dithering&&o.enable(14),S.transmission&&o.enable(15),S.sheen&&o.enable(16),S.opaque&&o.enable(17),S.pointsUvs&&o.enable(18),S.decodeVideoTexture&&o.enable(19),S.decodeVideoTextureEmissive&&o.enable(20),S.alphaToCoverage&&o.enable(21),S.numLightProbeGrids>0&&o.enable(22),S.hasPositionAttribute&&o.enable(23),g.push(o.mask)}function P(g){let S=p[g.type],_;if(S){let A=Mi[S];_=bl.clone(A.uniforms)}else _=g.uniforms;return _}function w(g,S){let _=h.get(S);return _!==void 0?++_.usedTimes:(_=new Nv(n,S,g,r),c.push(_),h.set(S,_)),_}function T(g){if(--g.usedTimes===0){let S=c.indexOf(g);c[S]=c[c.length-1],c.pop(),h.delete(g.cacheKey),g.destroy()}}function E(g){a.remove(g)}function D(){a.dispose()}return{getParameters:v,getProgramCacheKey:m,getUniforms:P,acquireProgram:w,releaseProgram:T,releaseShaderCache:E,programs:c,dispose:D}}function Bv(){let n=new WeakMap;function e(o){return n.has(o)}function t(o){let a=n.get(o);return a===void 0&&(a={},n.set(o,a)),a}function i(o){n.delete(o)}function r(o,a,l){n.get(o)[a]=l}function s(){n=new WeakMap}return{has:e,get:t,remove:i,update:r,dispose:s}}function Hv(n,e){return n.groupOrder!==e.groupOrder?n.groupOrder-e.groupOrder:n.renderOrder!==e.renderOrder?n.renderOrder-e.renderOrder:n.material.id!==e.material.id?n.material.id-e.material.id:n.materialVariant!==e.materialVariant?n.materialVariant-e.materialVariant:n.z!==e.z?n.z-e.z:n.id-e.id}function kd(n,e){return n.groupOrder!==e.groupOrder?n.groupOrder-e.groupOrder:n.renderOrder!==e.renderOrder?n.renderOrder-e.renderOrder:n.z!==e.z?e.z-n.z:n.id-e.id}function Ld(){let n=[],e=0,t=[],i=[],r=[];function s(){e=0,t.length=0,i.length=0,r.length=0}function o(u){let p=0;return u.isInstancedMesh&&(p+=2),u.isSkinnedMesh&&(p+=1),p}function a(u,p,x,v,m,f){let b=n[e];return b===void 0?(b={id:u.id,object:u,geometry:p,material:x,materialVariant:o(u),groupOrder:v,renderOrder:u.renderOrder,z:m,group:f},n[e]=b):(b.id=u.id,b.object=u,b.geometry=p,b.material=x,b.materialVariant=o(u),b.groupOrder=v,b.renderOrder=u.renderOrder,b.z=m,b.group=f),e++,b}function l(u,p,x,v,m,f,b){b.reversedDepth===!0&&(m=-m);let P=a(u,p,x,v,m,f);x.transmission>0?i.push(P):x.transparent===!0?r.push(P):t.push(P)}function c(u,p,x,v,m,f){let b=a(u,p,x,v,m,f);x.transmission>0?i.unshift(b):x.transparent===!0?r.unshift(b):t.unshift(b)}function h(u,p){t.length>1&&t.sort(u||Hv),i.length>1&&i.sort(p||kd),r.length>1&&r.sort(p||kd)}function d(){for(let u=e,p=n.length;u<p;u++){let x=n[u];if(x.id===null)break;x.id=null,x.object=null,x.geometry=null,x.material=null,x.group=null}}return{opaque:t,transmissive:i,transparent:r,init:s,push:l,unshift:c,finish:d,sort:h}}function zv(){let n=new WeakMap;function e(i,r){let s=n.get(i),o;return s===void 0?(o=new Ld,n.set(i,[o])):r>=s.length?(o=new Ld,s.push(o)):o=s[r],o}function t(){n=new WeakMap}return{get:e,dispose:t}}function Gv(){let n={};return{get:function(e){if(n[e.id]!==void 0)return n[e.id];let t;switch(e.type){case"SunLight":case"DirectionalLight":t={direction:new G,color:new ke};break;case"SpotLight":t={position:new G,direction:new G,color:new ke,distance:0,coneCos:0,penumbraCos:0,decay:0};break;case"PointLight":t={position:new G,color:new ke,distance:0,decay:0};break;case"HemisphereLight":t={direction:new G,skyColor:new ke,groundColor:new ke};break;case"RectAreaLight":t={color:new ke,position:new G,halfWidth:new G,halfHeight:new G};break}return n[e.id]=t,t}}}function Vv(){let n={};return{get:function(e){if(n[e.id]!==void 0)return n[e.id];let t;switch(e.type){case"SunLight":case"DirectionalLight":t={shadowIntensity:1,shadowBias:0,shadowNormalBias:0,shadowRadius:1,shadowMapSize:new Xe};break;case"SpotLight":t={shadowIntensity:1,shadowBias:0,shadowNormalBias:0,shadowRadius:1,shadowMapSize:new Xe};break;case"PointLight":t={shadowIntensity:1,shadowBias:0,shadowNormalBias:0,shadowRadius:1,shadowMapSize:new Xe,shadowCameraNear:1,shadowCameraFar:1e3};break}return n[e.id]=t,t}}}var Wv=0;function Xv(n,e){return(e.castShadow?2:0)-(n.castShadow?2:0)+(e.map?1:0)-(n.map?1:0)}function qv(n){let e=new Gv,t=Vv(),i={version:0,hash:{sunLength:-1,directionalLength:-1,pointLength:-1,spotLength:-1,rectAreaLength:-1,hemiLength:-1,numSunShadows:-1,numDirectionalShadows:-1,numPointShadows:-1,numSpotShadows:-1,numSpotMaps:-1,numLightProbes:-1},ambient:[0,0,0],probe:[],sun:[],sunShadow:[],sunShadowMap:[],sunShadowMatrix:[],sunShadowCascade:[],directional:[],directionalShadow:[],directionalShadowMap:[],directionalShadowMatrix:[],spot:[],spotLightMap:[],spotShadow:[],spotShadowMap:[],spotLightMatrix:[],rectArea:[],rectAreaLTC1:null,rectAreaLTC2:null,point:[],pointShadow:[],pointShadowMap:[],pointShadowMatrix:[],hemi:[],numSpotLightShadowsWithMaps:0,numLightProbes:0};for(let c=0;c<9;c++)i.probe.push(new G);let r=new G,s=new wt,o=new wt;function a(c){let h=0,d=0,u=0;for(let I=0;I<9;I++)i.probe[I].set(0,0,0);let p=0,x=0,v=0,m=0,f=0,b=0,P=0,w=0,T=0,E=0,D=0,g=0,S=0,_=0;c.sort(Xv);for(let I=0,k=c.length;I<k;I++){let N=c[I],U=N.color,z=N.intensity,Y=N.distance,ee=null;if(N.shadow&&N.shadow.map&&(N.shadow.map.texture.format===sr?ee=N.shadow.map.texture:ee=N.shadow.map.depthTexture||N.shadow.map.texture),N.isAmbientLight)h+=U.r*z,d+=U.g*z,u+=U.b*z;else if(N.isLightProbe){for(let J=0;J<9;J++)i.probe[J].addScaledVector(N.sh.coefficients[J],z);_++}else if(N.isSunLight){let J=e.get(N);if(J.color.copy(N.color).multiplyScalar(N.intensity),N.castShadow){let ie=N.shadow,ce=t.get(N);ce.shadowIntensity=ie.intensity,ce.shadowBias=ie.bias,ce.shadowNormalBias=ie.normalBias,ce.shadowRadius=ie.radius,ce.shadowMapSize.copy(ie.mapSize).multiply(ie.getFrameExtents()),i.sunShadow[x]=ce,i.sunShadowMap[x]=ee;let Se=ie.getViewportCount();for(let fe=0;fe<Se;fe++)i.sunShadowMatrix[v+fe]=ie.getMatrix(fe),i.sunShadowCascade[v+fe]=ie._cascadeData[fe];v+=Se,x++}i.sun[p]=J,p++}else if(N.isDirectionalLight){let J=e.get(N);if(J.color.copy(N.color).multiplyScalar(N.intensity),N.castShadow){let ie=N.shadow,ce=t.get(N);ce.shadowIntensity=ie.intensity,ce.shadowBias=ie.bias,ce.shadowNormalBias=ie.normalBias,ce.shadowRadius=ie.radius,ce.shadowMapSize=ie.mapSize,i.directionalShadow[m]=ce,i.directionalShadowMap[m]=ee,i.directionalShadowMatrix[m]=N.shadow.matrix,T++}i.directional[m]=J,m++}else if(N.isSpotLight){let J=e.get(N);J.position.setFromMatrixPosition(N.matrixWorld),J.color.copy(U).multiplyScalar(z),J.distance=Y,J.coneCos=Math.cos(N.angle),J.penumbraCos=Math.cos(N.angle*(1-N.penumbra)),J.decay=N.decay,i.spot[b]=J;let ie=N.shadow;if(N.map&&(i.spotLightMap[g]=N.map,g++,ie.updateMatrices(N),N.castShadow&&S++),i.spotLightMatrix[b]=ie.matrix,N.castShadow){let ce=t.get(N);ce.shadowIntensity=ie.intensity,ce.shadowBias=ie.bias,ce.shadowNormalBias=ie.normalBias,ce.shadowRadius=ie.radius,ce.shadowMapSize=ie.mapSize,i.spotShadow[b]=ce,i.spotShadowMap[b]=ee,D++}b++}else if(N.isRectAreaLight){let J=e.get(N);J.color.copy(U).multiplyScalar(z),J.halfWidth.set(N.width*.5,0,0),J.halfHeight.set(0,N.height*.5,0),i.rectArea[P]=J,P++}else if(N.isPointLight){let J=e.get(N);if(J.color.copy(N.color).multiplyScalar(N.intensity),J.distance=N.distance,J.decay=N.decay,N.castShadow){let ie=N.shadow,ce=t.get(N);ce.shadowIntensity=ie.intensity,ce.shadowBias=ie.bias,ce.shadowNormalBias=ie.normalBias,ce.shadowRadius=ie.radius,ce.shadowMapSize=ie.mapSize,ce.shadowCameraNear=ie.camera.near,ce.shadowCameraFar=ie.camera.far,i.pointShadow[f]=ce,i.pointShadowMap[f]=ee,i.pointShadowMatrix[f]=N.shadow.matrix,E++}i.point[f]=J,f++}else if(N.isHemisphereLight){let J=e.get(N);J.skyColor.copy(N.color).multiplyScalar(z),J.groundColor.copy(N.groundColor).multiplyScalar(z),i.hemi[w]=J,w++}}P>0&&(n.has("OES_texture_float_linear")===!0?(i.rectAreaLTC1=Te.LTC_FLOAT_1,i.rectAreaLTC2=Te.LTC_FLOAT_2):(i.rectAreaLTC1=Te.LTC_HALF_1,i.rectAreaLTC2=Te.LTC_HALF_2)),i.ambient[0]=h,i.ambient[1]=d,i.ambient[2]=u;let A=i.hash;(A.sunLength!==p||A.directionalLength!==m||A.pointLength!==f||A.spotLength!==b||A.rectAreaLength!==P||A.hemiLength!==w||A.numSunShadows!==x||A.numDirectionalShadows!==T||A.numPointShadows!==E||A.numSpotShadows!==D||A.numSpotMaps!==g||A.numLightProbes!==_)&&(i.sun.length=p,i.directional.length=m,i.spot.length=b,i.rectArea.length=P,i.point.length=f,i.hemi.length=w,i.sunShadow.length=x,i.sunShadowMap.length=x,i.sunShadowMatrix.length=v,i.sunShadowCascade.length=v,i.directionalShadow.length=T,i.directionalShadowMap.length=T,i.directionalShadowMatrix.length=T,i.pointShadow.length=E,i.pointShadowMap.length=E,i.pointShadowMatrix.length=E,i.spotShadow.length=D,i.spotShadowMap.length=D,i.spotLightMatrix.length=D+g-S,i.spotLightMap.length=g,i.numSpotLightShadowsWithMaps=S,i.numLightProbes=_,A.sunLength=p,A.directionalLength=m,A.pointLength=f,A.spotLength=b,A.rectAreaLength=P,A.hemiLength=w,A.numSunShadows=x,A.numDirectionalShadows=T,A.numPointShadows=E,A.numSpotShadows=D,A.numSpotMaps=g,A.numLightProbes=_,i.version=Wv++)}function l(c,h){let d=0,u=0,p=0,x=0,v=0,m=0,f=h.matrixWorldInverse;for(let b=0,P=c.length;b<P;b++){let w=c[b];if(w.isSunLight){let T=i.sun[d];T.direction.setFromMatrixPosition(w.matrixWorld),T.direction.transformDirection(f),d++}else if(w.isDirectionalLight){let T=i.directional[u];T.direction.setFromMatrixPosition(w.matrixWorld),r.setFromMatrixPosition(w.target.matrixWorld),T.direction.sub(r),T.direction.transformDirection(f),u++}else if(w.isSpotLight){let T=i.spot[x];T.position.setFromMatrixPosition(w.matrixWorld),T.position.applyMatrix4(f),T.direction.setFromMatrixPosition(w.matrixWorld),r.setFromMatrixPosition(w.target.matrixWorld),T.direction.sub(r),T.direction.transformDirection(f),x++}else if(w.isRectAreaLight){let T=i.rectArea[v];T.position.setFromMatrixPosition(w.matrixWorld),T.position.applyMatrix4(f),o.identity(),s.copy(w.matrixWorld),s.premultiply(f),o.extractRotation(s),T.halfWidth.set(w.width*.5,0,0),T.halfHeight.set(0,w.height*.5,0),T.halfWidth.applyMatrix4(o),T.halfHeight.applyMatrix4(o),v++}else if(w.isPointLight){let T=i.point[p];T.position.setFromMatrixPosition(w.matrixWorld),T.position.applyMatrix4(f),p++}else if(w.isHemisphereLight){let T=i.hemi[m];T.direction.setFromMatrixPosition(w.matrixWorld),T.direction.transformDirection(f),m++}}}return{setup:a,setupView:l,state:i}}function Dd(n){let e=new qv(n),t=[],i=[],r=[];function s(u){d.camera=u,t.length=0,i.length=0,r.length=0}function o(u){t.push(u)}function a(u){i.push(u)}function l(u){r.push(u)}function c(){e.setup(t)}function h(u){e.setupView(t,u)}let d={lightsArray:t,shadowsArray:i,lightProbeGridArray:r,camera:null,lights:e,transmissionRenderTarget:{},textureUnits:0};return{init:s,state:d,setupLights:c,setupLightsView:h,pushLight:o,pushShadow:a,pushLightProbeGrid:l}}function $v(n){let e=new WeakMap;function t(r,s=0){let o=e.get(r),a;return o===void 0?(a=new Dd(n),e.set(r,[a])):s>=o.length?(a=new Dd(n),o.push(a)):a=o[s],a}function i(){e=new WeakMap}return{get:t,dispose:i}}var Yv=`void main() {
	gl_Position = vec4( position, 1.0 );
}`,Zv=`uniform sampler2D shadow_pass;
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
}`,Kv=[new G(1,0,0),new G(-1,0,0),new G(0,1,0),new G(0,-1,0),new G(0,0,1),new G(0,0,-1)],Jv=[new G(0,-1,0),new G(0,-1,0),new G(0,0,1),new G(0,0,-1),new G(0,-1,0),new G(0,-1,0)],Nd=new wt,mo=new G,lu=new G;function jv(n,e,t){let i=new os,r=new Xe,s=new Xe,o=new Vt,a=new va,l=new ya,c={},h=t.maxTextureSize,d={[tr]:mn,[mn]:tr,[Fn]:Fn},u=new En({defines:{VSM_SAMPLES:8},uniforms:{shadow_pass:{value:null},resolution:{value:new Xe},radius:{value:4}},vertexShader:Yv,fragmentShader:Zv}),p=u.clone();p.defines.HORIZONTAL_PASS=1;let x=new Yt;x.setAttribute("position",new Cn(new Float32Array([-1,-1,.5,3,-1,.5,-1,3,.5]),3));let v=new $e(x,u),m=this;this.enabled=!1,this.autoUpdate=!0,this.needsUpdate=!1,this.type=Mr;let f=this.type;this.render=function(E,D,g){if(m.enabled===!1||m.autoUpdate===!1&&m.needsUpdate===!1||E.length===0)return;this.type===Ph&&(Ye("WebGLShadowMap: PCFSoftShadowMap has been removed. Using PCFShadowMap instead."),this.type=Mr);let S=n.getRenderTarget(),_=n.getActiveCubeFace(),A=n.getActiveMipmapLevel(),I=n.state;I.setBlending(_i),I.buffers.depth.getReversed()===!0?I.buffers.color.setClear(0,0,0,0):I.buffers.color.setClear(1,1,1,1),I.buffers.depth.setTest(!0),I.setScissorTest(!1);let k=f!==this.type;k&&D.traverse(function(N){N.material&&(Array.isArray(N.material)?N.material.forEach(U=>U.needsUpdate=!0):N.material.needsUpdate=!0)});for(let N=0,U=E.length;N<U;N++){let z=E[N],Y=z.shadow;if(Y===void 0){Ye("WebGLShadowMap:",z,"has no shadow.");continue}if(Y.autoUpdate===!1&&Y.needsUpdate===!1)continue;r.copy(Y.mapSize);let ee=Y.getFrameExtents();r.multiply(ee),s.copy(Y.mapSize),(r.x>h||r.y>h)&&(r.x>h&&(s.x=Math.floor(h/ee.x),r.x=s.x*ee.x,Y.mapSize.x=s.x),r.y>h&&(s.y=Math.floor(h/ee.y),r.y=s.y*ee.y,Y.mapSize.y=s.y));let J=n.state.buffers.depth.getReversed();if(Y.camera._reversedDepth=J,Y.map===null||k===!0){if(Y.map!==null&&(Y.map.depthTexture!==null&&(Y.map.depthTexture.dispose(),Y.map.depthTexture=null),Y.map.dispose()),this.type===hs){if(z.isPointLight){Ye("WebGLShadowMap: VSM shadow maps are not supported for PointLights. Use PCF or BasicShadowMap instead.");continue}Y.map=new Dn(r.x,r.y,{format:sr,type:hi,minFilter:vn,magFilter:vn,generateMipmaps:!1}),Y.map.texture.name=z.name+".shadowMap",Y.map.depthTexture=new Zi(r.x,r.y,Jn),Y.map.depthTexture.name=z.name+".shadowMapDepth",Y.map.depthTexture.format=vi,Y.map.depthTexture.compareFunction=null,Y.map.depthTexture.minFilter=en,Y.map.depthTexture.magFilter=en}else z.isPointLight?(Y.map=new El(r.x),Y.map.depthTexture=new ma(r.x,ui)):(Y.map=new Dn(r.x,r.y),Y.map.depthTexture=new Zi(r.x,r.y,ui)),Y.map.depthTexture.name=z.name+".shadowMap",Y.map.depthTexture.format=vi,this.type===Mr?(Y.map.depthTexture.compareFunction=J?_l:yl,Y.map.depthTexture.minFilter=vn,Y.map.depthTexture.magFilter=vn):(Y.map.depthTexture.compareFunction=null,Y.map.depthTexture.minFilter=en,Y.map.depthTexture.magFilter=en);Y.camera.updateProjectionMatrix()}Y.map.isWebGLCubeRenderTarget!==!0&&(Y.map.width!==r.x||Y.map.height!==r.y)&&Y.map.setSize(r.x,r.y);let ie=Y.map.isWebGLCubeRenderTarget?6:Y.getViewportCount();z.isPointLight!==!0&&Y.updateMatrices(z,g);for(let ce=0;ce<ie;ce++){let Se=Y.getCamera(ce);if(z.isPointLight){let fe=Y.camera,Ie=Y.matrix,We=z.distance||fe.far;We!==fe.far&&(fe.far=We,fe.updateProjectionMatrix()),mo.setFromMatrixPosition(z.matrixWorld),fe.position.copy(mo),lu.copy(fe.position),lu.add(Kv[ce]),fe.up.copy(Jv[ce]),fe.lookAt(lu),fe.updateMatrixWorld(),Ie.makeTranslation(-mo.x,-mo.y,-mo.z),Nd.multiplyMatrices(fe.projectionMatrix,fe.matrixWorldInverse),Y._frustum.setFromProjectionMatrix(Nd,fe.coordinateSystem,fe.reversedDepth)}if(Y.map.isWebGLCubeRenderTarget)n.setRenderTarget(Y.map,ce),n.clear();else{ce===0&&(n.setRenderTarget(Y.map),n.clear());let fe=Y.getViewport(ce);o.set(s.x*fe.x,s.y*fe.y,s.x*fe.z,s.y*fe.w),I.viewport(o)}i=Y.getFrustum(ce),w(D,g,Se,z,this.type)}Y.isPointLightShadow!==!0&&this.type===hs&&b(Y,g),Y.needsUpdate=!1}f=this.type,m.needsUpdate=!1,n.setRenderTarget(S,_,A)};function b(E,D){let g=e.update(v);u.defines.VSM_SAMPLES!==E.blurSamples&&(u.defines.VSM_SAMPLES=E.blurSamples,p.defines.VSM_SAMPLES=E.blurSamples,u.needsUpdate=!0,p.needsUpdate=!0),E.mapPass===null?E.mapPass=new Dn(r.x,r.y,{format:sr,type:hi}):(E.mapPass.width!==E.map.width||E.mapPass.height!==E.map.height)&&E.mapPass.setSize(E.map.width,E.map.height),u.uniforms.shadow_pass.value=E.map.depthTexture,u.uniforms.resolution.value.set(E.map.width,E.map.height),u.uniforms.radius.value=E.radius,n.setRenderTarget(E.mapPass),n.clear(),n.renderBufferDirect(D,null,g,u,v,null),p.uniforms.shadow_pass.value=E.mapPass.texture,p.uniforms.resolution.value.set(E.map.width,E.map.height),p.uniforms.radius.value=E.radius,n.setRenderTarget(E.map),n.clear(),n.renderBufferDirect(D,null,g,p,v,null)}function P(E,D,g,S){let _=null,A=g.isPointLight===!0?E.customDistanceMaterial:E.customDepthMaterial;if(A!==void 0)_=A;else if(_=g.isPointLight===!0?l:a,n.localClippingEnabled&&D.clipShadows===!0&&Array.isArray(D.clippingPlanes)&&D.clippingPlanes.length!==0||D.displacementMap&&D.displacementScale!==0||D.alphaMap&&D.alphaTest>0||D.map&&D.alphaTest>0||D.alphaToCoverage===!0){let I=_.uuid,k=D.uuid,N=c[I];N===void 0&&(N={},c[I]=N);let U=N[k];U===void 0&&(U=_.clone(),N[k]=U,D.addEventListener("dispose",T)),_=U}if(_.visible=D.visible,_.wireframe=D.wireframe,S===hs?_.side=D.shadowSide!==null?D.shadowSide:D.side:_.side=D.shadowSide!==null?D.shadowSide:d[D.side],_.alphaMap=D.alphaMap,_.alphaTest=D.alphaToCoverage===!0?.5:D.alphaTest,_.map=D.map,_.clipShadows=D.clipShadows,_.clippingPlanes=D.clippingPlanes,_.clipIntersection=D.clipIntersection,_.displacementMap=D.displacementMap,_.displacementScale=D.displacementScale,_.displacementBias=D.displacementBias,_.wireframeLinewidth=D.wireframeLinewidth,_.linewidth=D.linewidth,g.isPointLight===!0&&_.isMeshDistanceMaterial===!0){let I=n.properties.get(_);I.light=g}return _}function w(E,D,g,S,_){if(E.visible===!1)return;if(E.layers.test(D.layers)&&(E.isMesh||E.isLine||E.isPoints)&&(E.castShadow||E.receiveShadow&&_===hs)&&(!E.frustumCulled||E.intersectsFrustum(i))){E.modelViewMatrix.multiplyMatrices(g.matrixWorldInverse,E.matrixWorld);let k=e.update(E),N=E.material;if(Array.isArray(N)){let U=k.groups;for(let z=0,Y=U.length;z<Y;z++){let ee=U[z],J=N[ee.materialIndex];if(J&&J.visible){let ie=P(E,J,S,_);E.onBeforeShadow(n,E,D,g,k,ie,ee),n.renderBufferDirect(g,null,k,ie,E,ee),E.onAfterShadow(n,E,D,g,k,ie,ee)}}}else if(N.visible){let U=P(E,N,S,_);E.onBeforeShadow(n,E,D,g,k,U,null),n.renderBufferDirect(g,null,k,U,E,null),E.onAfterShadow(n,E,D,g,k,U,null)}}let I=E.children;for(let k=0,N=I.length;k<N;k++)w(I[k],D,g,S,_)}function T(E){E.target.removeEventListener("dispose",T);for(let g in c){let S=c[g],_=E.target.uuid;_ in S&&(S[_].dispose(),delete S[_])}}}function Qv(n,e){function t(){let B=!1,ue=new Vt,se=null,ge=new Vt(0,0,0,0);return{setMask:function(ye){se!==ye&&!B&&(n.colorMask(ye,ye,ye,ye),se=ye)},setLocked:function(ye){B=ye},setClear:function(ye,de,Pe,Le,vt){vt===!0&&(ye*=Le,de*=Le,Pe*=Le),ue.set(ye,de,Pe,Le),ge.equals(ue)===!1&&(n.clearColor(ye,de,Pe,Le),ge.copy(ue))},reset:function(){B=!1,se=null,ge.set(-1,0,0,0)}}}function i(){let B=!1,ue=!1,se=null,ge=null,ye=null;return{setReversed:function(de){if(ue!==de){let Pe=e.get("EXT_clip_control");de?Pe.clipControlEXT(Pe.LOWER_LEFT_EXT,Pe.ZERO_TO_ONE_EXT):Pe.clipControlEXT(Pe.LOWER_LEFT_EXT,Pe.NEGATIVE_ONE_TO_ONE_EXT),ue=de;let Le=ye;ye=null,this.setClear(Le)}},getReversed:function(){return ue},setTest:function(de){de?le(n.DEPTH_TEST):be(n.DEPTH_TEST)},setMask:function(de){se!==de&&!B&&(n.depthMask(de),se=de)},setFunc:function(de){if(ue&&(de=hd[de]),ge!==de){switch(de){case ia:n.depthFunc(n.NEVER);break;case ra:n.depthFunc(n.ALWAYS);break;case sa:n.depthFunc(n.LESS);break;case Qr:n.depthFunc(n.LEQUAL);break;case oa:n.depthFunc(n.EQUAL);break;case aa:n.depthFunc(n.GEQUAL);break;case la:n.depthFunc(n.GREATER);break;case ca:n.depthFunc(n.NOTEQUAL);break;default:n.depthFunc(n.LEQUAL)}ge=de}},setLocked:function(de){B=de},setClear:function(de){ye!==de&&(ye=de,ue&&(de=1-de),n.clearDepth(de))},reset:function(){B=!1,se=null,ge=null,ye=null,ue=!1}}}function r(){let B=!1,ue=null,se=null,ge=null,ye=null,de=null,Pe=null,Le=null,vt=null;return{setTest:function(ot){B||(ot?le(n.STENCIL_TEST):be(n.STENCIL_TEST))},setMask:function(ot){ue!==ot&&!B&&(n.stencilMask(ot),ue=ot)},setFunc:function(ot,Rt,an){(se!==ot||ge!==Rt||ye!==an)&&(n.stencilFunc(ot,Rt,an),se=ot,ge=Rt,ye=an)},setOp:function(ot,Rt,an){(de!==ot||Pe!==Rt||Le!==an)&&(n.stencilOp(ot,Rt,an),de=ot,Pe=Rt,Le=an)},setLocked:function(ot){B=ot},setClear:function(ot){vt!==ot&&(n.clearStencil(ot),vt=ot)},reset:function(){B=!1,ue=null,se=null,ge=null,ye=null,de=null,Pe=null,Le=null,vt=null}}}let s=new t,o=new i,a=new r,l=new WeakMap,c=new WeakMap,h={},d={},u={},p=new WeakMap,x=[],v=null,m=!1,f=null,b=null,P=null,w=null,T=null,E=null,D=null,g=new ke(0,0,0),S=0,_=!1,A=null,I=null,k=null,N=null,U=null,z=n.getParameter(n.MAX_COMBINED_TEXTURE_IMAGE_UNITS),Y=!1,ee=0,J=n.getParameter(n.VERSION);J.indexOf("WebGL")!==-1?(ee=parseFloat(/^WebGL (\d)/.exec(J)[1]),Y=ee>=1):J.indexOf("OpenGL ES")!==-1&&(ee=parseFloat(/^OpenGL ES (\d)/.exec(J)[1]),Y=ee>=2);let ie=null,ce={},Se=n.getParameter(n.SCISSOR_BOX),fe=n.getParameter(n.VIEWPORT),Ie=new Vt().fromArray(Se),We=new Vt().fromArray(fe);function He(B,ue,se,ge){let ye=new Uint8Array(4),de=n.createTexture();n.bindTexture(B,de),n.texParameteri(B,n.TEXTURE_MIN_FILTER,n.NEAREST),n.texParameteri(B,n.TEXTURE_MAG_FILTER,n.NEAREST);for(let Pe=0;Pe<se;Pe++)B===n.TEXTURE_3D||B===n.TEXTURE_2D_ARRAY?n.texImage3D(ue,0,n.RGBA,1,1,ge,0,n.RGBA,n.UNSIGNED_BYTE,ye):n.texImage2D(ue+Pe,0,n.RGBA,1,1,0,n.RGBA,n.UNSIGNED_BYTE,ye);return de}let oe={};oe[n.TEXTURE_2D]=He(n.TEXTURE_2D,n.TEXTURE_2D,1),oe[n.TEXTURE_CUBE_MAP]=He(n.TEXTURE_CUBE_MAP,n.TEXTURE_CUBE_MAP_POSITIVE_X,6),oe[n.TEXTURE_2D_ARRAY]=He(n.TEXTURE_2D_ARRAY,n.TEXTURE_2D_ARRAY,1,1),oe[n.TEXTURE_3D]=He(n.TEXTURE_3D,n.TEXTURE_3D,1,1),s.setClear(0,0,0,1),o.setClear(1),a.setClear(0),le(n.DEPTH_TEST),o.setFunc(Qr),tt(!1),Et(Cc),le(n.CULL_FACE),st(_i);function le(B){h[B]!==!0&&(n.enable(B),h[B]=!0)}function be(B){h[B]!==!1&&(n.disable(B),h[B]=!1)}function Ve(B,ue){return u[B]!==ue?(n.bindFramebuffer(B,ue),u[B]=ue,B===n.DRAW_FRAMEBUFFER&&(u[n.FRAMEBUFFER]=ue),B===n.FRAMEBUFFER&&(u[n.DRAW_FRAMEBUFFER]=ue),!0):!1}function Me(B,ue){let se=x,ge=!1;if(B){se=p.get(ue),se===void 0&&(se=[],p.set(ue,se));let ye=B.textures;if(se.length!==ye.length||se[0]!==n.COLOR_ATTACHMENT0){for(let de=0,Pe=ye.length;de<Pe;de++)se[de]=n.COLOR_ATTACHMENT0+de;se.length=ye.length,ge=!0}}else se[0]!==n.BACK&&(se[0]=n.BACK,ge=!0);ge&&n.drawBuffers(se)}function Ge(B){return v!==B?(n.useProgram(B),v=B,!0):!1}let Bt={[Sr]:n.FUNC_ADD,[kh]:n.FUNC_SUBTRACT,[Lh]:n.FUNC_REVERSE_SUBTRACT};Bt[Dh]=n.MIN,Bt[Nh]=n.MAX;let Je={[Uh]:n.ZERO,[Fh]:n.ONE,[Oh]:n.SRC_COLOR,[kc]:n.SRC_ALPHA,[Wh]:n.SRC_ALPHA_SATURATE,[Gh]:n.DST_COLOR,[Hh]:n.DST_ALPHA,[Bh]:n.ONE_MINUS_SRC_COLOR,[Lc]:n.ONE_MINUS_SRC_ALPHA,[Vh]:n.ONE_MINUS_DST_COLOR,[zh]:n.ONE_MINUS_DST_ALPHA,[Xh]:n.CONSTANT_COLOR,[qh]:n.ONE_MINUS_CONSTANT_COLOR,[$h]:n.CONSTANT_ALPHA,[Yh]:n.ONE_MINUS_CONSTANT_ALPHA};function st(B,ue,se,ge,ye,de,Pe,Le,vt,ot){if(B===_i){m===!0&&(be(n.BLEND),m=!1);return}if(m===!1&&(le(n.BLEND),m=!0),B!==Ih){if(B!==f||ot!==_){if((b!==Sr||T!==Sr)&&(n.blendEquation(n.FUNC_ADD),b=Sr,T=Sr),ot)switch(B){case ds:n.blendFuncSeparate(n.ONE,n.ONE_MINUS_SRC_ALPHA,n.ONE,n.ONE_MINUS_SRC_ALPHA);break;case Li:n.blendFunc(n.ONE,n.ONE);break;case Pc:n.blendFuncSeparate(n.ZERO,n.ONE_MINUS_SRC_COLOR,n.ZERO,n.ONE);break;case Ic:n.blendFuncSeparate(n.DST_COLOR,n.ONE_MINUS_SRC_ALPHA,n.ZERO,n.ONE);break;default:Ke("WebGLState: Invalid blending: ",B);break}else switch(B){case ds:n.blendFuncSeparate(n.SRC_ALPHA,n.ONE_MINUS_SRC_ALPHA,n.ONE,n.ONE_MINUS_SRC_ALPHA);break;case Li:n.blendFuncSeparate(n.SRC_ALPHA,n.ONE,n.ONE,n.ONE);break;case Pc:Ke("WebGLState: SubtractiveBlending requires material.premultipliedAlpha = true");break;case Ic:Ke("WebGLState: MultiplyBlending requires material.premultipliedAlpha = true");break;default:Ke("WebGLState: Invalid blending: ",B);break}P=null,w=null,E=null,D=null,g.set(0,0,0),S=0,f=B,_=ot}return}ye=ye||ue,de=de||se,Pe=Pe||ge,(ue!==b||ye!==T)&&(n.blendEquationSeparate(Bt[ue],Bt[ye]),b=ue,T=ye),(se!==P||ge!==w||de!==E||Pe!==D)&&(n.blendFuncSeparate(Je[se],Je[ge],Je[de],Je[Pe]),P=se,w=ge,E=de,D=Pe),(Le.equals(g)===!1||vt!==S)&&(n.blendColor(Le.r,Le.g,Le.b,vt),g.copy(Le),S=vt),f=B,_=!1}function mt(B,ue){B.side===Fn?be(n.CULL_FACE):le(n.CULL_FACE);let se=B.side===mn;ue&&(se=!se),tt(se),B.blending===ds&&B.transparent===!1?st(_i):st(B.blending,B.blendEquation,B.blendSrc,B.blendDst,B.blendEquationAlpha,B.blendSrcAlpha,B.blendDstAlpha,B.blendColor,B.blendAlpha,B.premultipliedAlpha),o.setFunc(B.depthFunc),o.setTest(B.depthTest),o.setMask(B.depthWrite),s.setMask(B.colorWrite);let ge=B.stencilWrite;a.setTest(ge),ge&&(a.setMask(B.stencilWriteMask),a.setFunc(B.stencilFunc,B.stencilRef,B.stencilFuncMask),a.setOp(B.stencilFail,B.stencilZFail,B.stencilZPass)),$t(B.polygonOffset,B.polygonOffsetFactor,B.polygonOffsetUnits),B.alphaToCoverage===!0?le(n.SAMPLE_ALPHA_TO_COVERAGE):be(n.SAMPLE_ALPHA_TO_COVERAGE)}function tt(B){A!==B&&(B?n.frontFace(n.CW):n.frontFace(n.CCW),A=B)}function Et(B){B!==Rh?(le(n.CULL_FACE),B!==I&&(B===Cc?n.cullFace(n.BACK):B===Ch?n.cullFace(n.FRONT):n.cullFace(n.FRONT_AND_BACK))):be(n.CULL_FACE),I=B}function Ht(B){B!==k&&(Y&&n.lineWidth(B),k=B)}function $t(B,ue,se){B?(le(n.POLYGON_OFFSET_FILL),(N!==ue||U!==se)&&(N=ue,U=se,o.getReversed()&&(ue=-ue),n.polygonOffset(ue,se))):be(n.POLYGON_OFFSET_FILL)}function It(B){B?le(n.SCISSOR_TEST):be(n.SCISSOR_TEST)}function zt(B){B===void 0&&(B=n.TEXTURE0+z-1),ie!==B&&(n.activeTexture(B),ie=B)}function W(B,ue,se){se===void 0&&(ie===null?se=n.TEXTURE0+z-1:se=ie);let ge=ce[se];ge===void 0&&(ge={type:void 0,texture:void 0},ce[se]=ge),(ge.type!==B||ge.texture!==ue)&&(ie!==se&&(n.activeTexture(se),ie=se),n.bindTexture(B,ue||oe[B]),ge.type=B,ge.texture=ue)}function nn(){let B=ce[ie];B!==void 0&&B.type!==void 0&&(n.bindTexture(B.type,null),B.type=void 0,B.texture=void 0)}function ut(){try{n.compressedTexImage2D(...arguments)}catch(B){Ke("WebGLState:",B)}}function F(){try{n.compressedTexImage3D(...arguments)}catch(B){Ke("WebGLState:",B)}}function M(){try{n.texSubImage2D(...arguments)}catch(B){Ke("WebGLState:",B)}}function Z(){try{n.texSubImage3D(...arguments)}catch(B){Ke("WebGLState:",B)}}function te(){try{n.compressedTexSubImage2D(...arguments)}catch(B){Ke("WebGLState:",B)}}function ae(){try{n.compressedTexSubImage3D(...arguments)}catch(B){Ke("WebGLState:",B)}}function ve(){try{n.texStorage2D(...arguments)}catch(B){Ke("WebGLState:",B)}}function _e(){try{n.texStorage3D(...arguments)}catch(B){Ke("WebGLState:",B)}}function C(){try{n.texImage2D(...arguments)}catch(B){Ke("WebGLState:",B)}}function L(){try{n.texImage3D(...arguments)}catch(B){Ke("WebGLState:",B)}}function O(B){return d[B]!==void 0?d[B]:n.getParameter(B)}function H(B,ue){d[B]!==ue&&(n.pixelStorei(B,ue),d[B]=ue)}function q(B){Ie.equals(B)===!1&&(n.scissor(B.x,B.y,B.z,B.w),Ie.copy(B))}function K(B){We.equals(B)===!1&&(n.viewport(B.x,B.y,B.z,B.w),We.copy(B))}function re(B,ue){let se=c.get(ue);se===void 0&&(se=new WeakMap,c.set(ue,se));let ge=se.get(B);ge===void 0&&(ge=n.getUniformBlockIndex(ue,B.name),se.set(B,ge))}function pe(B,ue){let ge=c.get(ue).get(B);l.get(ue)!==ge&&(n.uniformBlockBinding(ue,ge,B.__bindingPointIndex),l.set(ue,ge))}function me(){n.disable(n.BLEND),n.disable(n.CULL_FACE),n.disable(n.DEPTH_TEST),n.disable(n.POLYGON_OFFSET_FILL),n.disable(n.SCISSOR_TEST),n.disable(n.STENCIL_TEST),n.disable(n.SAMPLE_ALPHA_TO_COVERAGE),n.blendEquation(n.FUNC_ADD),n.blendFunc(n.ONE,n.ZERO),n.blendFuncSeparate(n.ONE,n.ZERO,n.ONE,n.ZERO),n.blendColor(0,0,0,0),n.colorMask(!0,!0,!0,!0),n.clearColor(0,0,0,0),n.depthMask(!0),n.depthFunc(n.LESS),o.setReversed(!1),n.clearDepth(1),n.stencilMask(4294967295),n.stencilFunc(n.ALWAYS,0,4294967295),n.stencilOp(n.KEEP,n.KEEP,n.KEEP),n.clearStencil(0),n.cullFace(n.BACK),n.frontFace(n.CCW),n.polygonOffset(0,0),n.activeTexture(n.TEXTURE0),n.bindFramebuffer(n.FRAMEBUFFER,null),n.bindFramebuffer(n.DRAW_FRAMEBUFFER,null),n.bindFramebuffer(n.READ_FRAMEBUFFER,null),n.useProgram(null),n.lineWidth(1),n.scissor(0,0,n.canvas.width,n.canvas.height),n.viewport(0,0,n.canvas.width,n.canvas.height),n.pixelStorei(n.PACK_ALIGNMENT,4),n.pixelStorei(n.UNPACK_ALIGNMENT,4),n.pixelStorei(n.UNPACK_FLIP_Y_WEBGL,!1),n.pixelStorei(n.UNPACK_PREMULTIPLY_ALPHA_WEBGL,!1),n.pixelStorei(n.UNPACK_COLORSPACE_CONVERSION_WEBGL,n.BROWSER_DEFAULT_WEBGL),n.pixelStorei(n.PACK_ROW_LENGTH,0),n.pixelStorei(n.PACK_SKIP_PIXELS,0),n.pixelStorei(n.PACK_SKIP_ROWS,0),n.pixelStorei(n.UNPACK_ROW_LENGTH,0),n.pixelStorei(n.UNPACK_IMAGE_HEIGHT,0),n.pixelStorei(n.UNPACK_SKIP_PIXELS,0),n.pixelStorei(n.UNPACK_SKIP_ROWS,0),n.pixelStorei(n.UNPACK_SKIP_IMAGES,0),h={},d={},ie=null,ce={},u={},p=new WeakMap,x=[],v=null,m=!1,f=null,b=null,P=null,w=null,T=null,E=null,D=null,g=new ke(0,0,0),S=0,_=!1,A=null,I=null,k=null,N=null,U=null,Ie.set(0,0,n.canvas.width,n.canvas.height),We.set(0,0,n.canvas.width,n.canvas.height),s.reset(),o.reset(),a.reset()}return{buffers:{color:s,depth:o,stencil:a},enable:le,disable:be,bindFramebuffer:Ve,drawBuffers:Me,useProgram:Ge,setBlending:st,setMaterial:mt,setFlipSided:tt,setCullFace:Et,setLineWidth:Ht,setPolygonOffset:$t,setScissorTest:It,activeTexture:zt,bindTexture:W,unbindTexture:nn,compressedTexImage2D:ut,compressedTexImage3D:F,texImage2D:C,texImage3D:L,pixelStorei:H,getParameter:O,updateUBOMapping:re,uniformBlockBinding:pe,texStorage2D:ve,texStorage3D:_e,texSubImage2D:M,texSubImage3D:Z,compressedTexSubImage2D:te,compressedTexSubImage3D:ae,scissor:q,viewport:K,reset:me}}function e1(n,e,t,i,r,s,o){let a=e.has("WEBGL_multisampled_render_to_texture")?e.get("WEBGL_multisampled_render_to_texture"):null,l=typeof navigator>"u"?!1:/OculusBrowser/g.test(navigator.userAgent),c=new Xe,h=new WeakMap,d=new Set,u,p=new WeakMap,x=!1;try{x=typeof OffscreenCanvas<"u"&&new OffscreenCanvas(1,1).getContext("2d")!==null}catch{}function v(F,M){return x?new OffscreenCanvas(F,M):Hs("canvas")}function m(F,M,Z){let te=1,ae=ut(F);if((ae.width>Z||ae.height>Z)&&(te=Z/Math.max(ae.width,ae.height)),te<1)if(typeof HTMLImageElement<"u"&&F instanceof HTMLImageElement||typeof HTMLCanvasElement<"u"&&F instanceof HTMLCanvasElement||typeof ImageBitmap<"u"&&F instanceof ImageBitmap||typeof VideoFrame<"u"&&F instanceof VideoFrame){let ve=Math.floor(te*ae.width),_e=Math.floor(te*ae.height);u===void 0&&(u=v(ve,_e));let C=M?v(ve,_e):u;return C.width=ve,C.height=_e,C.getContext("2d").drawImage(F,0,0,ve,_e),Ye("WebGLRenderer: Texture has been resized from ("+ae.width+"x"+ae.height+") to ("+ve+"x"+_e+")."),C}else return"data"in F&&Ye("WebGLRenderer: Image in DataTexture is too big ("+ae.width+"x"+ae.height+")."),F;return F}function f(F){return F.generateMipmaps}function b(F){n.generateMipmap(F)}function P(F){return F.isWebGLCubeRenderTarget?n.TEXTURE_CUBE_MAP:F.isWebGL3DRenderTarget?n.TEXTURE_3D:F.isWebGLArrayRenderTarget||F.isCompressedArrayTexture?n.TEXTURE_2D_ARRAY:n.TEXTURE_2D}function w(F,M,Z,te,ae,ve=!1){if(F!==null){if(n[F]!==void 0)return n[F];Ye("WebGLRenderer: Attempt to use non-existing WebGL internal format '"+F+"'")}let _e;te&&(_e=e.get("EXT_texture_norm16"),_e||Ye("WebGLRenderer: Unable to use normalized textures without EXT_texture_norm16 extension"));let C=M;if(M===n.RED&&(Z===n.FLOAT&&(C=n.R32F),Z===n.HALF_FLOAT&&(C=n.R16F),Z===n.UNSIGNED_BYTE&&(C=n.R8),Z===n.UNSIGNED_SHORT&&_e&&(C=_e.R16_EXT),Z===n.SHORT&&_e&&(C=_e.R16_SNORM_EXT)),M===n.RED_INTEGER&&(Z===n.UNSIGNED_BYTE&&(C=n.R8UI),Z===n.UNSIGNED_SHORT&&(C=n.R16UI),Z===n.UNSIGNED_INT&&(C=n.R32UI),Z===n.BYTE&&(C=n.R8I),Z===n.SHORT&&(C=n.R16I),Z===n.INT&&(C=n.R32I)),M===n.RG&&(Z===n.FLOAT&&(C=n.RG32F),Z===n.HALF_FLOAT&&(C=n.RG16F),Z===n.UNSIGNED_BYTE&&(C=n.RG8),Z===n.UNSIGNED_SHORT&&_e&&(C=_e.RG16_EXT),Z===n.SHORT&&_e&&(C=_e.RG16_SNORM_EXT)),M===n.RG_INTEGER&&(Z===n.UNSIGNED_BYTE&&(C=n.RG8UI),Z===n.UNSIGNED_SHORT&&(C=n.RG16UI),Z===n.UNSIGNED_INT&&(C=n.RG32UI),Z===n.BYTE&&(C=n.RG8I),Z===n.SHORT&&(C=n.RG16I),Z===n.INT&&(C=n.RG32I)),M===n.RGB_INTEGER&&(Z===n.UNSIGNED_BYTE&&(C=n.RGB8UI),Z===n.UNSIGNED_SHORT&&(C=n.RGB16UI),Z===n.UNSIGNED_INT&&(C=n.RGB32UI),Z===n.BYTE&&(C=n.RGB8I),Z===n.SHORT&&(C=n.RGB16I),Z===n.INT&&(C=n.RGB32I)),M===n.RGBA_INTEGER&&(Z===n.UNSIGNED_BYTE&&(C=n.RGBA8UI),Z===n.UNSIGNED_SHORT&&(C=n.RGBA16UI),Z===n.UNSIGNED_INT&&(C=n.RGBA32UI),Z===n.BYTE&&(C=n.RGBA8I),Z===n.SHORT&&(C=n.RGBA16I),Z===n.INT&&(C=n.RGBA32I)),M===n.RGB&&(Z===n.UNSIGNED_SHORT&&_e&&(C=_e.RGB16_EXT),Z===n.SHORT&&_e&&(C=_e.RGB16_SNORM_EXT),Z===n.UNSIGNED_INT_5_9_9_9_REV&&(C=n.RGB9_E5),Z===n.UNSIGNED_INT_10F_11F_11F_REV&&(C=n.R11F_G11F_B10F)),M===n.RGBA){let L=ve?Bs:ht.getTransfer(ae);Z===n.FLOAT&&(C=n.RGBA32F),Z===n.HALF_FLOAT&&(C=n.RGBA16F),Z===n.UNSIGNED_BYTE&&(C=L===St?n.SRGB8_ALPHA8:n.RGBA8),Z===n.UNSIGNED_SHORT&&_e&&(C=_e.RGBA16_EXT),Z===n.SHORT&&_e&&(C=_e.RGBA16_SNORM_EXT),Z===n.UNSIGNED_SHORT_4_4_4_4&&(C=n.RGBA4),Z===n.UNSIGNED_SHORT_5_5_5_1&&(C=n.RGB5_A1)}return(C===n.R16F||C===n.R32F||C===n.RG16F||C===n.RG32F||C===n.RGBA16F||C===n.RGBA32F)&&e.get("EXT_color_buffer_float"),C}function T(F,M){let Z;return F?M===null||M===ui||M===ps?Z=n.DEPTH24_STENCIL8:M===Jn?Z=n.DEPTH32F_STENCIL8:M===fs&&(Z=n.DEPTH24_STENCIL8,Ye("DepthTexture: 16 bit depth attachment is not supported with stencil. Using 24-bit attachment.")):M===null||M===ui||M===ps?Z=n.DEPTH_COMPONENT24:M===Jn?Z=n.DEPTH_COMPONENT32F:M===fs&&(Z=n.DEPTH_COMPONENT16),Z}function E(F,M){return f(F)===!0||F.isFramebufferTexture&&F.minFilter!==en&&F.minFilter!==vn?Math.log2(Math.max(M.width,M.height))+1:F.mipmaps!==void 0&&F.mipmaps.length>0?F.mipmaps.length:F.isCompressedTexture&&Array.isArray(F.image)?M.mipmaps.length:1}function D(F){let M=F.target;M.removeEventListener("dispose",D),S(M),M.isVideoTexture&&h.delete(M),M.isHTMLTexture&&d.delete(M)}function g(F){let M=F.target;M.removeEventListener("dispose",g),A(M)}function S(F){let M=i.get(F);if(M.__webglInit===void 0)return;let Z=F.source,te=p.get(Z);if(te){let ae=te[M.__cacheKey];ae.usedTimes--,ae.usedTimes===0&&_(F),Object.keys(te).length===0&&p.delete(Z)}i.remove(F)}function _(F){let M=i.get(F);n.deleteTexture(M.__webglTexture);let Z=F.source,te=p.get(Z);delete te[M.__cacheKey],o.memory.textures--}function A(F){let M=i.get(F);if(F.depthTexture&&(F.depthTexture.dispose(),i.remove(F.depthTexture)),F.isWebGLCubeRenderTarget)for(let te=0;te<6;te++){if(Array.isArray(M.__webglFramebuffer[te]))for(let ae=0;ae<M.__webglFramebuffer[te].length;ae++)n.deleteFramebuffer(M.__webglFramebuffer[te][ae]);else n.deleteFramebuffer(M.__webglFramebuffer[te]);M.__webglDepthbuffer&&n.deleteRenderbuffer(M.__webglDepthbuffer[te])}else{if(Array.isArray(M.__webglFramebuffer))for(let te=0;te<M.__webglFramebuffer.length;te++)n.deleteFramebuffer(M.__webglFramebuffer[te]);else n.deleteFramebuffer(M.__webglFramebuffer);if(M.__webglDepthbuffer&&n.deleteRenderbuffer(M.__webglDepthbuffer),M.__webglMultisampledFramebuffer&&n.deleteFramebuffer(M.__webglMultisampledFramebuffer),M.__webglColorRenderbuffer)for(let te=0;te<M.__webglColorRenderbuffer.length;te++)M.__webglColorRenderbuffer[te]&&n.deleteRenderbuffer(M.__webglColorRenderbuffer[te]);M.__webglDepthRenderbuffer&&n.deleteRenderbuffer(M.__webglDepthRenderbuffer)}let Z=F.textures;for(let te=0,ae=Z.length;te<ae;te++){let ve=i.get(Z[te]);ve.__webglTexture&&(n.deleteTexture(ve.__webglTexture),o.memory.textures--),i.remove(Z[te])}i.remove(F)}let I=0;function k(){I=0}function N(){return I}function U(F){I=F}function z(){let F=I;return F>=r.maxTextures&&Ye("WebGLTextures: Trying to use "+(F+1)+" texture units while this GPU supports only "+r.maxTextures),I+=1,F}function Y(F){let M=[];return M.push(F.wrapS),M.push(F.wrapT),M.push(F.wrapR||0),M.push(F.magFilter),M.push(F.minFilter),M.push(F.anisotropy),M.push(F.internalFormat),M.push(F.format),M.push(F.type),M.push(F.generateMipmaps),M.push(F.premultiplyAlpha),M.push(F.flipY),M.push(F.unpackAlignment),M.push(F.colorSpace),M.join()}function ee(F,M){let Z=i.get(F);if(F.isVideoTexture&&W(F),F.isRenderTargetTexture===!1&&F.isExternalTexture!==!0&&F.version>0&&Z.__version!==F.version){let te=F.image;if(te===null)Ye("WebGLRenderer: Texture marked for update but no image data found.");else if(te.complete===!1)Ye("WebGLRenderer: Texture marked for update but image is incomplete");else{be(Z,F,M);return}}else F.isExternalTexture&&(Z.__webglTexture=F.sourceTexture?F.sourceTexture:null);t.bindTexture(n.TEXTURE_2D,Z.__webglTexture,n.TEXTURE0+M)}function J(F,M){let Z=i.get(F);if(F.isRenderTargetTexture===!1&&F.version>0&&Z.__version!==F.version){be(Z,F,M);return}else F.isExternalTexture&&(Z.__webglTexture=F.sourceTexture?F.sourceTexture:null);t.bindTexture(n.TEXTURE_2D_ARRAY,Z.__webglTexture,n.TEXTURE0+M)}function ie(F,M){let Z=i.get(F);if(F.isRenderTargetTexture===!1&&F.version>0&&Z.__version!==F.version){be(Z,F,M);return}t.bindTexture(n.TEXTURE_3D,Z.__webglTexture,n.TEXTURE0+M)}function ce(F,M){let Z=i.get(F);if(F.isCubeDepthTexture!==!0&&F.version>0&&Z.__version!==F.version){Ve(Z,F,M);return}t.bindTexture(n.TEXTURE_CUBE_MAP,Z.__webglTexture,n.TEXTURE0+M)}let Se={[gr]:n.REPEAT,[xi]:n.CLAMP_TO_EDGE,[ua]:n.MIRRORED_REPEAT},fe={[en]:n.NEAREST,[Jh]:n.NEAREST_MIPMAP_NEAREST,[oo]:n.NEAREST_MIPMAP_LINEAR,[vn]:n.LINEAR,[Da]:n.LINEAR_MIPMAP_NEAREST,[ir]:n.LINEAR_MIPMAP_LINEAR},Ie={[td]:n.NEVER,[od]:n.ALWAYS,[nd]:n.LESS,[yl]:n.LEQUAL,[id]:n.EQUAL,[_l]:n.GEQUAL,[rd]:n.GREATER,[sd]:n.NOTEQUAL};function We(F,M){if(M.type===Jn&&e.has("OES_texture_float_linear")===!1&&(M.magFilter===vn||M.magFilter===Da||M.magFilter===oo||M.magFilter===ir||M.minFilter===vn||M.minFilter===Da||M.minFilter===oo||M.minFilter===ir)&&Ye("WebGLRenderer: Unable to use linear filtering with floating point textures. OES_texture_float_linear not supported on this device."),n.texParameteri(F,n.TEXTURE_WRAP_S,Se[M.wrapS]),n.texParameteri(F,n.TEXTURE_WRAP_T,Se[M.wrapT]),(F===n.TEXTURE_3D||F===n.TEXTURE_2D_ARRAY)&&n.texParameteri(F,n.TEXTURE_WRAP_R,Se[M.wrapR]),n.texParameteri(F,n.TEXTURE_MAG_FILTER,fe[M.magFilter]),n.texParameteri(F,n.TEXTURE_MIN_FILTER,fe[M.minFilter]),M.compareFunction&&(n.texParameteri(F,n.TEXTURE_COMPARE_MODE,n.COMPARE_REF_TO_TEXTURE),n.texParameteri(F,n.TEXTURE_COMPARE_FUNC,Ie[M.compareFunction])),e.has("EXT_texture_filter_anisotropic")===!0){if(M.magFilter===en||M.minFilter!==oo&&M.minFilter!==ir||M.type===Jn&&e.has("OES_texture_float_linear")===!1)return;if(M.anisotropy>1||i.get(M).__currentAnisotropy){let Z=e.get("EXT_texture_filter_anisotropic");n.texParameterf(F,Z.TEXTURE_MAX_ANISOTROPY_EXT,Math.min(M.anisotropy,r.getMaxAnisotropy())),i.get(M).__currentAnisotropy=M.anisotropy}}}function He(F,M){let Z=!1;F.__webglInit===void 0&&(F.__webglInit=!0,M.addEventListener("dispose",D));let te=M.source,ae=p.get(te);ae===void 0&&(ae={},p.set(te,ae));let ve=Y(M);if(ve!==F.__cacheKey){ae[ve]===void 0&&(ae[ve]={texture:n.createTexture(),usedTimes:0},o.memory.textures++,Z=!0),ae[ve].usedTimes++;let _e=ae[F.__cacheKey];_e!==void 0&&(ae[F.__cacheKey].usedTimes--,_e.usedTimes===0&&_(M)),F.__cacheKey=ve,F.__webglTexture=ae[ve].texture}return Z}function oe(F,M,Z){return Math.floor(Math.floor(F/Z)/M)}function le(F,M,Z,te){let ve=F.updateRanges;if(ve.length===0)t.texSubImage2D(n.TEXTURE_2D,0,0,0,M.width,M.height,Z,te,M.data);else{ve.sort((H,q)=>H.start-q.start);let _e=0;for(let H=1;H<ve.length;H++){let q=ve[_e],K=ve[H],re=q.start+q.count,pe=oe(K.start,M.width,4),me=oe(q.start,M.width,4);K.start<=re+1&&pe===me&&oe(K.start+K.count-1,M.width,4)===pe?q.count=Math.max(q.count,K.start+K.count-q.start):(++_e,ve[_e]=K)}ve.length=_e+1;let C=t.getParameter(n.UNPACK_ROW_LENGTH),L=t.getParameter(n.UNPACK_SKIP_PIXELS),O=t.getParameter(n.UNPACK_SKIP_ROWS);t.pixelStorei(n.UNPACK_ROW_LENGTH,M.width);for(let H=0,q=ve.length;H<q;H++){let K=ve[H],re=Math.floor(K.start/4),pe=Math.ceil(K.count/4),me=re%M.width,B=Math.floor(re/M.width),ue=pe,se=1;t.pixelStorei(n.UNPACK_SKIP_PIXELS,me),t.pixelStorei(n.UNPACK_SKIP_ROWS,B),t.texSubImage2D(n.TEXTURE_2D,0,me,B,ue,se,Z,te,M.data)}F.clearUpdateRanges(),t.pixelStorei(n.UNPACK_ROW_LENGTH,C),t.pixelStorei(n.UNPACK_SKIP_PIXELS,L),t.pixelStorei(n.UNPACK_SKIP_ROWS,O)}}function be(F,M,Z){let te=n.TEXTURE_2D;(M.isDataArrayTexture||M.isCompressedArrayTexture)&&(te=n.TEXTURE_2D_ARRAY),M.isData3DTexture&&(te=n.TEXTURE_3D);let ae=He(F,M),ve=M.source;t.bindTexture(te,F.__webglTexture,n.TEXTURE0+Z);let _e=i.get(ve);if(ve.version!==_e.__version||ae===!0){if(t.activeTexture(n.TEXTURE0+Z),(typeof ImageBitmap<"u"&&M.image instanceof ImageBitmap)===!1){let se=ht.getPrimaries(ht.workingColorSpace),ge=M.colorSpace===Di?null:ht.getPrimaries(M.colorSpace),ye=M.colorSpace===Di||se===ge?n.NONE:n.BROWSER_DEFAULT_WEBGL;t.pixelStorei(n.UNPACK_FLIP_Y_WEBGL,M.flipY),t.pixelStorei(n.UNPACK_PREMULTIPLY_ALPHA_WEBGL,M.premultiplyAlpha),t.pixelStorei(n.UNPACK_COLORSPACE_CONVERSION_WEBGL,ye)}t.pixelStorei(n.UNPACK_ALIGNMENT,M.unpackAlignment);let L=m(M.image,!1,r.maxTextureSize);L=nn(M,L);let O=s.convert(M.format,M.colorSpace),H=s.convert(M.type),q=w(M.internalFormat,O,H,M.normalized,M.colorSpace,M.isVideoTexture);We(te,M);let K,re=M.mipmaps,pe=M.isVideoTexture!==!0,me=_e.__version===void 0||ae===!0,B=ve.dataReady,ue=E(M,L);if(M.isDepthTexture)q=T(M.format===rr,M.type),me&&(pe?t.texStorage2D(n.TEXTURE_2D,1,q,L.width,L.height):t.texImage2D(n.TEXTURE_2D,0,q,L.width,L.height,0,O,H,null));else if(M.isDataTexture)if(re.length>0){pe&&me&&t.texStorage2D(n.TEXTURE_2D,ue,q,re[0].width,re[0].height);for(let se=0,ge=re.length;se<ge;se++)K=re[se],pe?B&&t.texSubImage2D(n.TEXTURE_2D,se,0,0,K.width,K.height,O,H,K.data):t.texImage2D(n.TEXTURE_2D,se,q,K.width,K.height,0,O,H,K.data);M.generateMipmaps=!1}else pe?(me&&t.texStorage2D(n.TEXTURE_2D,ue,q,L.width,L.height),B&&le(M,L,O,H)):t.texImage2D(n.TEXTURE_2D,0,q,L.width,L.height,0,O,H,L.data);else if(M.isCompressedTexture)if(M.isCompressedArrayTexture){pe&&me&&t.texStorage3D(n.TEXTURE_2D_ARRAY,ue,q,re[0].width,re[0].height,L.depth);for(let se=0,ge=re.length;se<ge;se++)if(K=re[se],M.format!==Bn)if(O!==null)if(pe){if(B)if(M.layerUpdates.size>0){let ye=nu(K.width,K.height,M.format,M.type);for(let de of M.layerUpdates){let Pe=K.data.subarray(de*ye/K.data.BYTES_PER_ELEMENT,(de+1)*ye/K.data.BYTES_PER_ELEMENT);t.compressedTexSubImage3D(n.TEXTURE_2D_ARRAY,se,0,0,de,K.width,K.height,1,O,Pe)}}else t.compressedTexSubImage3D(n.TEXTURE_2D_ARRAY,se,0,0,0,K.width,K.height,L.depth,O,K.data)}else t.compressedTexImage3D(n.TEXTURE_2D_ARRAY,se,q,K.width,K.height,L.depth,0,K.data,0,0);else Ye("WebGLRenderer: Attempt to load unsupported compressed texture format in .uploadTexture()");else pe?B&&t.texSubImage3D(n.TEXTURE_2D_ARRAY,se,0,0,0,K.width,K.height,L.depth,O,H,K.data):t.texImage3D(n.TEXTURE_2D_ARRAY,se,q,K.width,K.height,L.depth,0,O,H,K.data);M.layerUpdates.size>0&&M.clearLayerUpdates()}else{pe&&me&&t.texStorage2D(n.TEXTURE_2D,ue,q,re[0].width,re[0].height);for(let se=0,ge=re.length;se<ge;se++)K=re[se],M.format!==Bn?O!==null?pe?B&&t.compressedTexSubImage2D(n.TEXTURE_2D,se,0,0,K.width,K.height,O,K.data):t.compressedTexImage2D(n.TEXTURE_2D,se,q,K.width,K.height,0,K.data):Ye("WebGLRenderer: Attempt to load unsupported compressed texture format in .uploadTexture()"):pe?B&&t.texSubImage2D(n.TEXTURE_2D,se,0,0,K.width,K.height,O,H,K.data):t.texImage2D(n.TEXTURE_2D,se,q,K.width,K.height,0,O,H,K.data)}else if(M.isDataArrayTexture)if(pe){if(me&&t.texStorage3D(n.TEXTURE_2D_ARRAY,ue,q,L.width,L.height,L.depth),B)if(M.layerUpdates.size>0){let se=nu(L.width,L.height,M.format,M.type);for(let ge of M.layerUpdates){let ye=L.data.subarray(ge*se/L.data.BYTES_PER_ELEMENT,(ge+1)*se/L.data.BYTES_PER_ELEMENT);t.texSubImage3D(n.TEXTURE_2D_ARRAY,0,0,0,ge,L.width,L.height,1,O,H,ye)}M.clearLayerUpdates()}else t.texSubImage3D(n.TEXTURE_2D_ARRAY,0,0,0,0,L.width,L.height,L.depth,O,H,L.data)}else t.texImage3D(n.TEXTURE_2D_ARRAY,0,q,L.width,L.height,L.depth,0,O,H,L.data);else if(M.isData3DTexture)pe?(me&&t.texStorage3D(n.TEXTURE_3D,ue,q,L.width,L.height,L.depth),B&&t.texSubImage3D(n.TEXTURE_3D,0,0,0,0,L.width,L.height,L.depth,O,H,L.data)):t.texImage3D(n.TEXTURE_3D,0,q,L.width,L.height,L.depth,0,O,H,L.data);else if(M.isFramebufferTexture){if(me)if(pe)t.texStorage2D(n.TEXTURE_2D,ue,q,L.width,L.height);else{let se=L.width,ge=L.height;for(let ye=0;ye<ue;ye++)t.texImage2D(n.TEXTURE_2D,ye,q,se,ge,0,O,H,null),se>>=1,ge>>=1}}else if(M.isHTMLTexture){if("texElementImage2D"in n){let se=n.canvas;if(se.hasAttribute("layoutsubtree")||se.setAttribute("layoutsubtree","true"),L.parentNode!==se){se.appendChild(L),d.add(M),se.onpaint=ge=>{let ye=ge.changedElements;for(let de of d)ye.includes(de.image)&&(de.needsUpdate=!0)},se.requestPaint();return}if(n.texElementImage2D.length===3)n.texElementImage2D(n.TEXTURE_2D,n.RGBA8,L);else{let ye=n.RGBA,de=n.RGBA,Pe=n.UNSIGNED_BYTE;n.texElementImage2D(n.TEXTURE_2D,0,ye,de,Pe,L)}n.texParameteri(n.TEXTURE_2D,n.TEXTURE_MIN_FILTER,n.LINEAR),n.texParameteri(n.TEXTURE_2D,n.TEXTURE_WRAP_S,n.CLAMP_TO_EDGE),n.texParameteri(n.TEXTURE_2D,n.TEXTURE_WRAP_T,n.CLAMP_TO_EDGE)}}else if(re.length>0){if(pe&&me){let se=ut(re[0]);t.texStorage2D(n.TEXTURE_2D,ue,q,se.width,se.height)}for(let se=0,ge=re.length;se<ge;se++)K=re[se],pe?B&&t.texSubImage2D(n.TEXTURE_2D,se,0,0,O,H,K):t.texImage2D(n.TEXTURE_2D,se,q,O,H,K);M.generateMipmaps=!1}else if(pe){if(me){let se=ut(L);t.texStorage2D(n.TEXTURE_2D,ue,q,se.width,se.height)}B&&t.texSubImage2D(n.TEXTURE_2D,0,0,0,O,H,L)}else t.texImage2D(n.TEXTURE_2D,0,q,O,H,L);f(M)&&b(te),_e.__version=ve.version,M.onUpdate&&M.onUpdate(M)}F.__version=M.version}function Ve(F,M,Z){if(M.image.length!==6)return;let te=He(F,M),ae=M.source;t.bindTexture(n.TEXTURE_CUBE_MAP,F.__webglTexture,n.TEXTURE0+Z);let ve=i.get(ae);if(ae.version!==ve.__version||te===!0){t.activeTexture(n.TEXTURE0+Z);let _e=ht.getPrimaries(ht.workingColorSpace),C=M.colorSpace===Di?null:ht.getPrimaries(M.colorSpace),L=M.colorSpace===Di||_e===C?n.NONE:n.BROWSER_DEFAULT_WEBGL;t.pixelStorei(n.UNPACK_FLIP_Y_WEBGL,M.flipY),t.pixelStorei(n.UNPACK_PREMULTIPLY_ALPHA_WEBGL,M.premultiplyAlpha),t.pixelStorei(n.UNPACK_ALIGNMENT,M.unpackAlignment),t.pixelStorei(n.UNPACK_COLORSPACE_CONVERSION_WEBGL,L);let O=M.isCompressedTexture||M.image[0].isCompressedTexture,H=M.image[0]&&M.image[0].isDataTexture,q=[];for(let de=0;de<6;de++)!O&&!H?q[de]=m(M.image[de],!0,r.maxCubemapSize):q[de]=H?M.image[de].image:M.image[de],q[de]=nn(M,q[de]);let K=q[0],re=s.convert(M.format,M.colorSpace),pe=s.convert(M.type),me=w(M.internalFormat,re,pe,M.normalized,M.colorSpace),B=M.isVideoTexture!==!0,ue=ve.__version===void 0||te===!0,se=ae.dataReady,ge=E(M,K);We(n.TEXTURE_CUBE_MAP,M);let ye;if(O){B&&ue&&t.texStorage2D(n.TEXTURE_CUBE_MAP,ge,me,K.width,K.height);for(let de=0;de<6;de++){ye=q[de].mipmaps;for(let Pe=0;Pe<ye.length;Pe++){let Le=ye[Pe];M.format!==Bn?re!==null?B?se&&t.compressedTexSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,Pe,0,0,Le.width,Le.height,re,Le.data):t.compressedTexImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,Pe,me,Le.width,Le.height,0,Le.data):Ye("WebGLRenderer: Attempt to load unsupported compressed texture format in .setTextureCube()"):B?se&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,Pe,0,0,Le.width,Le.height,re,pe,Le.data):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,Pe,me,Le.width,Le.height,0,re,pe,Le.data)}}}else{if(ye=M.mipmaps,B&&ue){ye.length>0&&ge++;let de=ut(q[0]);t.texStorage2D(n.TEXTURE_CUBE_MAP,ge,me,de.width,de.height)}for(let de=0;de<6;de++)if(H){B?se&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,0,0,0,q[de].width,q[de].height,re,pe,q[de].data):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,0,me,q[de].width,q[de].height,0,re,pe,q[de].data);for(let Pe=0;Pe<ye.length;Pe++){let vt=ye[Pe].image[de].image;B?se&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,Pe+1,0,0,vt.width,vt.height,re,pe,vt.data):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,Pe+1,me,vt.width,vt.height,0,re,pe,vt.data)}}else{B?se&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,0,0,0,re,pe,q[de]):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,0,me,re,pe,q[de]);for(let Pe=0;Pe<ye.length;Pe++){let Le=ye[Pe];B?se&&t.texSubImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,Pe+1,0,0,re,pe,Le.image[de]):t.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+de,Pe+1,me,re,pe,Le.image[de])}}}f(M)&&b(n.TEXTURE_CUBE_MAP),ve.__version=ae.version,M.onUpdate&&M.onUpdate(M)}F.__version=M.version}function Me(F,M,Z,te,ae,ve){let _e=s.convert(Z.format,Z.colorSpace),C=s.convert(Z.type),L=w(Z.internalFormat,_e,C,Z.normalized,Z.colorSpace),O=i.get(M),H=i.get(Z);if(H.__renderTarget=M,!O.__hasExternalTextures){let q=Math.max(1,M.width>>ve),K=Math.max(1,M.height>>ve);ae===n.TEXTURE_3D||ae===n.TEXTURE_2D_ARRAY?t.texImage3D(ae,ve,L,q,K,M.depth,0,_e,C,null):t.texImage2D(ae,ve,L,q,K,0,_e,C,null)}t.bindFramebuffer(n.FRAMEBUFFER,F),zt(M)?a.framebufferTexture2DMultisampleEXT(n.FRAMEBUFFER,te,ae,H.__webglTexture,0,It(M)):(ae===n.TEXTURE_2D||ae>=n.TEXTURE_CUBE_MAP_POSITIVE_X&&ae<=n.TEXTURE_CUBE_MAP_NEGATIVE_Z)&&n.framebufferTexture2D(n.FRAMEBUFFER,te,ae,H.__webglTexture,ve),t.bindFramebuffer(n.FRAMEBUFFER,null)}function Ge(F,M,Z){if(n.bindRenderbuffer(n.RENDERBUFFER,F),M.depthBuffer){let te=M.depthTexture,ae=te&&te.isDepthTexture?te.type:null,ve=T(M.stencilBuffer,ae),_e=M.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT;zt(M)?a.renderbufferStorageMultisampleEXT(n.RENDERBUFFER,It(M),ve,M.width,M.height):Z?n.renderbufferStorageMultisample(n.RENDERBUFFER,It(M),ve,M.width,M.height):n.renderbufferStorage(n.RENDERBUFFER,ve,M.width,M.height),n.framebufferRenderbuffer(n.FRAMEBUFFER,_e,n.RENDERBUFFER,F)}else{let te=M.textures;for(let ae=0;ae<te.length;ae++){let ve=te[ae],_e=s.convert(ve.format,ve.colorSpace),C=s.convert(ve.type),L=w(ve.internalFormat,_e,C,ve.normalized,ve.colorSpace);zt(M)?a.renderbufferStorageMultisampleEXT(n.RENDERBUFFER,It(M),L,M.width,M.height):Z?n.renderbufferStorageMultisample(n.RENDERBUFFER,It(M),L,M.width,M.height):n.renderbufferStorage(n.RENDERBUFFER,L,M.width,M.height)}}n.bindRenderbuffer(n.RENDERBUFFER,null)}function Bt(F,M,Z){let te=M.isWebGLCubeRenderTarget===!0;if(t.bindFramebuffer(n.FRAMEBUFFER,F),!(M.depthTexture&&M.depthTexture.isDepthTexture))throw new Error("THREE.WebGLTextures: renderTarget.depthTexture must be an instance of THREE.DepthTexture.");let ae=i.get(M.depthTexture);if(ae.__renderTarget=M,(!ae.__webglTexture||M.depthTexture.image.width!==M.width||M.depthTexture.image.height!==M.height)&&(M.depthTexture.image.width=M.width,M.depthTexture.image.height=M.height,M.depthTexture.needsUpdate=!0),te){if(ae.__webglInit===void 0&&(ae.__webglInit=!0,M.depthTexture.addEventListener("dispose",D)),ae.__webglTexture===void 0){ae.__webglTexture=n.createTexture(),t.bindTexture(n.TEXTURE_CUBE_MAP,ae.__webglTexture),We(n.TEXTURE_CUBE_MAP,M.depthTexture);let O=s.convert(M.depthTexture.format),H=s.convert(M.depthTexture.type),q;M.depthTexture.format===vi?q=n.DEPTH_COMPONENT24:M.depthTexture.format===rr&&(q=n.DEPTH24_STENCIL8);for(let K=0;K<6;K++)n.texImage2D(n.TEXTURE_CUBE_MAP_POSITIVE_X+K,0,q,M.width,M.height,0,O,H,null)}}else ee(M.depthTexture,0);let ve=ae.__webglTexture,_e=It(M),C=te?n.TEXTURE_CUBE_MAP_POSITIVE_X+Z:n.TEXTURE_2D,L=M.depthTexture.format===rr?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT;if(M.depthTexture.format===vi)zt(M)?a.framebufferTexture2DMultisampleEXT(n.FRAMEBUFFER,L,C,ve,0,_e):n.framebufferTexture2D(n.FRAMEBUFFER,L,C,ve,0);else if(M.depthTexture.format===rr)zt(M)?a.framebufferTexture2DMultisampleEXT(n.FRAMEBUFFER,L,C,ve,0,_e):n.framebufferTexture2D(n.FRAMEBUFFER,L,C,ve,0);else throw new Error("THREE.WebGLTextures: Unknown depthTexture format.")}function Je(F){let M=i.get(F),Z=F.isWebGLCubeRenderTarget===!0;if(M.__boundDepthTexture!==F.depthTexture){let te=F.depthTexture;if(M.__depthDisposeCallback&&M.__depthDisposeCallback(),te){let ae=()=>{delete M.__boundDepthTexture,delete M.__depthDisposeCallback,te.removeEventListener("dispose",ae)};te.addEventListener("dispose",ae),M.__depthDisposeCallback=ae}M.__boundDepthTexture=te}if(F.depthTexture&&!M.__autoAllocateDepthBuffer)if(Z)for(let te=0;te<6;te++)Bt(M.__webglFramebuffer[te],F,te);else{let te=F.texture.mipmaps;te&&te.length>0?Bt(M.__webglFramebuffer[0],F,0):Bt(M.__webglFramebuffer,F,0)}else if(Z){M.__webglDepthbuffer=[];for(let te=0;te<6;te++)if(t.bindFramebuffer(n.FRAMEBUFFER,M.__webglFramebuffer[te]),M.__webglDepthbuffer[te]===void 0)M.__webglDepthbuffer[te]=n.createRenderbuffer(),Ge(M.__webglDepthbuffer[te],F,!1);else{let ae=F.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT,ve=M.__webglDepthbuffer[te];n.bindRenderbuffer(n.RENDERBUFFER,ve),n.framebufferRenderbuffer(n.FRAMEBUFFER,ae,n.RENDERBUFFER,ve)}}else{let te=F.texture.mipmaps;if(te&&te.length>0?t.bindFramebuffer(n.FRAMEBUFFER,M.__webglFramebuffer[0]):t.bindFramebuffer(n.FRAMEBUFFER,M.__webglFramebuffer),M.__webglDepthbuffer===void 0)M.__webglDepthbuffer=n.createRenderbuffer(),Ge(M.__webglDepthbuffer,F,!1);else{let ae=F.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT,ve=M.__webglDepthbuffer;n.bindRenderbuffer(n.RENDERBUFFER,ve),n.framebufferRenderbuffer(n.FRAMEBUFFER,ae,n.RENDERBUFFER,ve)}}t.bindFramebuffer(n.FRAMEBUFFER,null)}function st(F,M,Z){let te=i.get(F);M!==void 0&&Me(te.__webglFramebuffer,F,F.texture,n.COLOR_ATTACHMENT0,n.TEXTURE_2D,0),Z!==void 0&&Je(F)}function mt(F){let M=F.texture,Z=i.get(F),te=i.get(M);F.addEventListener("dispose",g);let ae=F.textures,ve=F.isWebGLCubeRenderTarget===!0,_e=ae.length>1;if(_e||(te.__webglTexture===void 0&&(te.__webglTexture=n.createTexture()),te.__version=M.version,o.memory.textures++),ve){Z.__webglFramebuffer=[];for(let C=0;C<6;C++)if(M.mipmaps&&M.mipmaps.length>0){Z.__webglFramebuffer[C]=[];for(let L=0;L<M.mipmaps.length;L++)Z.__webglFramebuffer[C][L]=n.createFramebuffer()}else Z.__webglFramebuffer[C]=n.createFramebuffer()}else{if(M.mipmaps&&M.mipmaps.length>0){Z.__webglFramebuffer=[];for(let C=0;C<M.mipmaps.length;C++)Z.__webglFramebuffer[C]=n.createFramebuffer()}else Z.__webglFramebuffer=n.createFramebuffer();if(_e)for(let C=0,L=ae.length;C<L;C++){let O=i.get(ae[C]);O.__webglTexture===void 0&&(O.__webglTexture=n.createTexture(),o.memory.textures++)}if(F.samples>0&&zt(F)===!1){Z.__webglMultisampledFramebuffer=n.createFramebuffer(),Z.__webglColorRenderbuffer=[],t.bindFramebuffer(n.FRAMEBUFFER,Z.__webglMultisampledFramebuffer);for(let C=0;C<ae.length;C++){let L=ae[C];Z.__webglColorRenderbuffer[C]=n.createRenderbuffer(),n.bindRenderbuffer(n.RENDERBUFFER,Z.__webglColorRenderbuffer[C]);let O=s.convert(L.format,L.colorSpace),H=s.convert(L.type),q=w(L.internalFormat,O,H,L.normalized,L.colorSpace,F.isXRRenderTarget===!0),K=It(F);n.renderbufferStorageMultisample(n.RENDERBUFFER,K,q,F.width,F.height),n.framebufferRenderbuffer(n.FRAMEBUFFER,n.COLOR_ATTACHMENT0+C,n.RENDERBUFFER,Z.__webglColorRenderbuffer[C])}n.bindRenderbuffer(n.RENDERBUFFER,null),F.depthBuffer&&(Z.__webglDepthRenderbuffer=n.createRenderbuffer(),Ge(Z.__webglDepthRenderbuffer,F,!0)),t.bindFramebuffer(n.FRAMEBUFFER,null)}}if(ve){t.bindTexture(n.TEXTURE_CUBE_MAP,te.__webglTexture),We(n.TEXTURE_CUBE_MAP,M);for(let C=0;C<6;C++)if(M.mipmaps&&M.mipmaps.length>0)for(let L=0;L<M.mipmaps.length;L++)Me(Z.__webglFramebuffer[C][L],F,M,n.COLOR_ATTACHMENT0,n.TEXTURE_CUBE_MAP_POSITIVE_X+C,L);else Me(Z.__webglFramebuffer[C],F,M,n.COLOR_ATTACHMENT0,n.TEXTURE_CUBE_MAP_POSITIVE_X+C,0);f(M)&&b(n.TEXTURE_CUBE_MAP),t.unbindTexture()}else if(_e){for(let C=0,L=ae.length;C<L;C++){let O=ae[C],H=i.get(O),q=n.TEXTURE_2D;(F.isWebGL3DRenderTarget||F.isWebGLArrayRenderTarget)&&(q=F.isWebGL3DRenderTarget?n.TEXTURE_3D:n.TEXTURE_2D_ARRAY),t.bindTexture(q,H.__webglTexture),We(q,O),Me(Z.__webglFramebuffer,F,O,n.COLOR_ATTACHMENT0+C,q,0),f(O)&&b(q)}t.unbindTexture()}else{let C=n.TEXTURE_2D;if((F.isWebGL3DRenderTarget||F.isWebGLArrayRenderTarget)&&(C=F.isWebGL3DRenderTarget?n.TEXTURE_3D:n.TEXTURE_2D_ARRAY),t.bindTexture(C,te.__webglTexture),We(C,M),M.mipmaps&&M.mipmaps.length>0)for(let L=0;L<M.mipmaps.length;L++)Me(Z.__webglFramebuffer[L],F,M,n.COLOR_ATTACHMENT0,C,L);else Me(Z.__webglFramebuffer,F,M,n.COLOR_ATTACHMENT0,C,0);f(M)&&b(C),t.unbindTexture()}F.depthBuffer&&Je(F)}function tt(F){let M=F.textures;for(let Z=0,te=M.length;Z<te;Z++){let ae=M[Z];if(f(ae)){let ve=P(F),_e=i.get(ae).__webglTexture;t.bindTexture(ve,_e),b(ve),t.unbindTexture()}}}let Et=[],Ht=[];function $t(F){if(F.samples>0){if(zt(F)===!1){let M=F.textures,Z=F.width,te=F.height,ae=n.COLOR_BUFFER_BIT,ve=F.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT,_e=i.get(F),C=M.length>1;if(C)for(let O=0;O<M.length;O++)t.bindFramebuffer(n.FRAMEBUFFER,_e.__webglMultisampledFramebuffer),n.framebufferRenderbuffer(n.FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.RENDERBUFFER,null),t.bindFramebuffer(n.FRAMEBUFFER,_e.__webglFramebuffer),n.framebufferTexture2D(n.DRAW_FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.TEXTURE_2D,null,0);t.bindFramebuffer(n.READ_FRAMEBUFFER,_e.__webglMultisampledFramebuffer);let L=F.texture.mipmaps;L&&L.length>0?t.bindFramebuffer(n.DRAW_FRAMEBUFFER,_e.__webglFramebuffer[0]):t.bindFramebuffer(n.DRAW_FRAMEBUFFER,_e.__webglFramebuffer);for(let O=0;O<M.length;O++){if(F.resolveDepthBuffer&&(F.depthBuffer&&(ae|=n.DEPTH_BUFFER_BIT),F.stencilBuffer&&F.resolveStencilBuffer&&(ae|=n.STENCIL_BUFFER_BIT)),C){n.framebufferRenderbuffer(n.READ_FRAMEBUFFER,n.COLOR_ATTACHMENT0,n.RENDERBUFFER,_e.__webglColorRenderbuffer[O]);let H=i.get(M[O]).__webglTexture;n.framebufferTexture2D(n.DRAW_FRAMEBUFFER,n.COLOR_ATTACHMENT0,n.TEXTURE_2D,H,0)}n.blitFramebuffer(0,0,Z,te,0,0,Z,te,ae,n.NEAREST),l===!0&&(Et.length=0,Ht.length=0,Et.push(n.COLOR_ATTACHMENT0+O),F.depthBuffer&&F.storeMultisampledDepthBuffer===!1&&(Et.push(ve),Ht.push(ve),n.invalidateFramebuffer(n.DRAW_FRAMEBUFFER,Ht)),n.invalidateFramebuffer(n.READ_FRAMEBUFFER,Et))}if(t.bindFramebuffer(n.READ_FRAMEBUFFER,null),t.bindFramebuffer(n.DRAW_FRAMEBUFFER,null),C)for(let O=0;O<M.length;O++){t.bindFramebuffer(n.FRAMEBUFFER,_e.__webglMultisampledFramebuffer),n.framebufferRenderbuffer(n.FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.RENDERBUFFER,_e.__webglColorRenderbuffer[O]);let H=i.get(M[O]).__webglTexture;t.bindFramebuffer(n.FRAMEBUFFER,_e.__webglFramebuffer),n.framebufferTexture2D(n.DRAW_FRAMEBUFFER,n.COLOR_ATTACHMENT0+O,n.TEXTURE_2D,H,0)}t.bindFramebuffer(n.DRAW_FRAMEBUFFER,_e.__webglMultisampledFramebuffer)}else if(F.depthBuffer&&F.storeMultisampledDepthBuffer===!1&&l){let M=F.stencilBuffer?n.DEPTH_STENCIL_ATTACHMENT:n.DEPTH_ATTACHMENT;n.invalidateFramebuffer(n.DRAW_FRAMEBUFFER,[M])}}}function It(F){return Math.min(r.maxSamples,F.samples)}function zt(F){let M=i.get(F);return F.samples>0&&e.has("WEBGL_multisampled_render_to_texture")===!0&&M.__useRenderToTexture!==!1}function W(F){let M=o.render.frame;h.get(F)!==M&&(h.set(F,M),F.update())}function nn(F,M){let Z=F.colorSpace,te=F.format,ae=F.type;return F.isCompressedTexture===!0||F.isVideoTexture===!0||Z!==Os&&Z!==Di&&(ht.getTransfer(Z)===St?(te!==Bn||ae!==On)&&Ye("WebGLTextures: sRGB encoded textures have to use RGBAFormat and UnsignedByteType."):Ke("WebGLTextures: Unsupported texture color space:",Z)),M}function ut(F){return typeof HTMLImageElement<"u"&&F instanceof HTMLImageElement?(c.width=F.naturalWidth||F.width,c.height=F.naturalHeight||F.height):typeof VideoFrame<"u"&&F instanceof VideoFrame?(c.width=F.displayWidth,c.height=F.displayHeight):(c.width=F.width,c.height=F.height),c}this.allocateTextureUnit=z,this.resetTextureUnits=k,this.getTextureUnits=N,this.setTextureUnits=U,this.setTexture2D=ee,this.setTexture2DArray=J,this.setTexture3D=ie,this.setTextureCube=ce,this.rebindTextures=st,this.setupRenderTarget=mt,this.updateRenderTargetMipmap=tt,this.updateMultisampleRenderTarget=$t,this.setupDepthRenderbuffer=Je,this.setupFrameBufferTexture=Me,this.useMultisampledRTT=zt,this.isReversedDepthBuffer=function(){return t.buffers.depth.getReversed()}}function t1(n,e){function t(i,r=Di){let s,o=ht.getTransfer(r);if(i===On)return n.UNSIGNED_BYTE;if(i===Ua)return n.UNSIGNED_SHORT_4_4_4_4;if(i===Fa)return n.UNSIGNED_SHORT_5_5_5_1;if(i===Xc)return n.UNSIGNED_INT_5_9_9_9_REV;if(i===qc)return n.UNSIGNED_INT_10F_11F_11F_REV;if(i===Vc)return n.BYTE;if(i===Wc)return n.SHORT;if(i===fs)return n.UNSIGNED_SHORT;if(i===Na)return n.INT;if(i===ui)return n.UNSIGNED_INT;if(i===Jn)return n.FLOAT;if(i===hi)return n.HALF_FLOAT;if(i===$c)return n.ALPHA;if(i===Yc)return n.RGB;if(i===Bn)return n.RGBA;if(i===vi)return n.DEPTH_COMPONENT;if(i===rr)return n.DEPTH_STENCIL;if(i===Oa)return n.RED;if(i===Ba)return n.RED_INTEGER;if(i===sr)return n.RG;if(i===Ha)return n.RG_INTEGER;if(i===za)return n.RGBA_INTEGER;if(i===ao||i===lo||i===co||i===uo)if(o===St)if(s=e.get("WEBGL_compressed_texture_s3tc_srgb"),s!==null){if(i===ao)return s.COMPRESSED_SRGB_S3TC_DXT1_EXT;if(i===lo)return s.COMPRESSED_SRGB_ALPHA_S3TC_DXT1_EXT;if(i===co)return s.COMPRESSED_SRGB_ALPHA_S3TC_DXT3_EXT;if(i===uo)return s.COMPRESSED_SRGB_ALPHA_S3TC_DXT5_EXT}else return null;else if(s=e.get("WEBGL_compressed_texture_s3tc"),s!==null){if(i===ao)return s.COMPRESSED_RGB_S3TC_DXT1_EXT;if(i===lo)return s.COMPRESSED_RGBA_S3TC_DXT1_EXT;if(i===co)return s.COMPRESSED_RGBA_S3TC_DXT3_EXT;if(i===uo)return s.COMPRESSED_RGBA_S3TC_DXT5_EXT}else return null;if(i===Ga||i===Va||i===Wa||i===Xa)if(s=e.get("WEBGL_compressed_texture_pvrtc"),s!==null){if(i===Ga)return s.COMPRESSED_RGB_PVRTC_4BPPV1_IMG;if(i===Va)return s.COMPRESSED_RGB_PVRTC_2BPPV1_IMG;if(i===Wa)return s.COMPRESSED_RGBA_PVRTC_4BPPV1_IMG;if(i===Xa)return s.COMPRESSED_RGBA_PVRTC_2BPPV1_IMG}else return null;if(i===qa||i===$a||i===Ya||i===Za||i===Ka||i===ho||i===Ja)if(s=e.get("WEBGL_compressed_texture_etc"),s!==null){if(i===qa||i===$a)return o===St?s.COMPRESSED_SRGB8_ETC2:s.COMPRESSED_RGB8_ETC2;if(i===Ya)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ETC2_EAC:s.COMPRESSED_RGBA8_ETC2_EAC;if(i===Za)return s.COMPRESSED_R11_EAC;if(i===Ka)return s.COMPRESSED_SIGNED_R11_EAC;if(i===ho)return s.COMPRESSED_RG11_EAC;if(i===Ja)return s.COMPRESSED_SIGNED_RG11_EAC}else return null;if(i===ja||i===Qa||i===el||i===tl||i===nl||i===il||i===rl||i===sl||i===ol||i===al||i===ll||i===cl||i===ul||i===hl)if(s=e.get("WEBGL_compressed_texture_astc"),s!==null){if(i===ja)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_4x4_KHR:s.COMPRESSED_RGBA_ASTC_4x4_KHR;if(i===Qa)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_5x4_KHR:s.COMPRESSED_RGBA_ASTC_5x4_KHR;if(i===el)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_5x5_KHR:s.COMPRESSED_RGBA_ASTC_5x5_KHR;if(i===tl)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_6x5_KHR:s.COMPRESSED_RGBA_ASTC_6x5_KHR;if(i===nl)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_6x6_KHR:s.COMPRESSED_RGBA_ASTC_6x6_KHR;if(i===il)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_8x5_KHR:s.COMPRESSED_RGBA_ASTC_8x5_KHR;if(i===rl)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_8x6_KHR:s.COMPRESSED_RGBA_ASTC_8x6_KHR;if(i===sl)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_8x8_KHR:s.COMPRESSED_RGBA_ASTC_8x8_KHR;if(i===ol)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x5_KHR:s.COMPRESSED_RGBA_ASTC_10x5_KHR;if(i===al)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x6_KHR:s.COMPRESSED_RGBA_ASTC_10x6_KHR;if(i===ll)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x8_KHR:s.COMPRESSED_RGBA_ASTC_10x8_KHR;if(i===cl)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_10x10_KHR:s.COMPRESSED_RGBA_ASTC_10x10_KHR;if(i===ul)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_12x10_KHR:s.COMPRESSED_RGBA_ASTC_12x10_KHR;if(i===hl)return o===St?s.COMPRESSED_SRGB8_ALPHA8_ASTC_12x12_KHR:s.COMPRESSED_RGBA_ASTC_12x12_KHR}else return null;if(i===dl||i===fl||i===pl)if(s=e.get("EXT_texture_compression_bptc"),s!==null){if(i===dl)return o===St?s.COMPRESSED_SRGB_ALPHA_BPTC_UNORM_EXT:s.COMPRESSED_RGBA_BPTC_UNORM_EXT;if(i===fl)return s.COMPRESSED_RGB_BPTC_SIGNED_FLOAT_EXT;if(i===pl)return s.COMPRESSED_RGB_BPTC_UNSIGNED_FLOAT_EXT}else return null;if(i===ml||i===gl||i===fo||i===xl)if(s=e.get("EXT_texture_compression_rgtc"),s!==null){if(i===ml)return s.COMPRESSED_RED_RGTC1_EXT;if(i===gl)return s.COMPRESSED_SIGNED_RED_RGTC1_EXT;if(i===fo)return s.COMPRESSED_RED_GREEN_RGTC2_EXT;if(i===xl)return s.COMPRESSED_SIGNED_RED_GREEN_RGTC2_EXT}else return null;return i===ps?n.UNSIGNED_INT_24_8:n[i]!==void 0?n[i]:null}return{convert:t}}var n1=`
void main() {

	gl_Position = vec4( position, 1.0 );

}`,i1=`
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

}`,gu=class{constructor(){this.texture=null,this.mesh=null,this.depthNear=0,this.depthFar=0}init(e,t){if(this.texture===null){let i=new Zs(e.texture);(e.depthNear!==t.depthNear||e.depthFar!==t.depthFar)&&(this.depthNear=e.depthNear,this.depthFar=e.depthFar),this.texture=i}}getMesh(e){if(this.texture!==null&&this.mesh===null){let t=e.cameras[0].viewport,i=new En({vertexShader:n1,fragmentShader:i1,uniforms:{depthColor:{value:this.texture},depthWidth:{value:t.z},depthHeight:{value:t.w}}});this.mesh=new $e(new Xn(20,20),i)}return this.mesh}reset(){this.texture=null,this.mesh=null}getDepthTexture(){return this.texture}},xu=class extends li{constructor(e,t){super();let i=this,r=null,s=1,o=null,a="local-floor",l=1,c=null,h=null,d=null,u=null,p=null,x=null,v=typeof XRWebGLBinding<"u",m=new gu,f={},b=t.getContextAttributes(),P=null,w=null,T=[],E=[],D=new Xe,g=null,S=null,_=new cn;_.viewport=new Vt;let A=new cn;A.viewport=new Vt;let I=[_,A],k=new Ia,N=null,U=null;this.cameraAutoUpdate=!0,this.enabled=!1,this.isPresenting=!1,this.getController=function(oe){let le=T[oe];return le===void 0&&(le=new ss,T[oe]=le),le.getTargetRaySpace()},this.getControllerGrip=function(oe){let le=T[oe];return le===void 0&&(le=new ss,T[oe]=le),le.getGripSpace()},this.getHand=function(oe){let le=T[oe];return le===void 0&&(le=new ss,T[oe]=le),le.getHandSpace()};function z(oe){let le=E.indexOf(oe.inputSource);if(le===-1)return;let be=T[le];be!==void 0&&(be.update(oe.inputSource,oe.frame,c||o),be.dispatchEvent({type:oe.type,data:oe.inputSource}))}function Y(){r.removeEventListener("select",z),r.removeEventListener("selectstart",z),r.removeEventListener("selectend",z),r.removeEventListener("squeeze",z),r.removeEventListener("squeezestart",z),r.removeEventListener("squeezeend",z),r.removeEventListener("end",Y),r.removeEventListener("inputsourceschange",ee);for(let oe=0;oe<T.length;oe++){let le=E[oe];le!==null&&(E[oe]=null,T[oe].disconnect(le))}N=null,U=null,m.reset();for(let oe in f)delete f[oe];if(e.setRenderTarget(P),p=null,u=null,d=null,r=null,w=null,He.stop(),i.isPresenting=!1,e.setPixelRatio(g),e.setSize(D.width,D.height,!1),S!==null){let oe=S.camera;oe.fov=S.fov,oe.zoom=S.zoom,oe.updateProjectionMatrix(),S=null}i.dispatchEvent({type:"sessionend"})}this.setFramebufferScaleFactor=function(oe){s=oe,i.isPresenting===!0&&Ye("WebXRManager: Cannot change framebuffer scale while presenting.")},this.setReferenceSpaceType=function(oe){a=oe,i.isPresenting===!0&&Ye("WebXRManager: Cannot change reference space type while presenting.")},this.getReferenceSpace=function(){return c||o},this.setReferenceSpace=function(oe){c=oe},this.getBaseLayer=function(){return u!==null?u:p},this.getBinding=function(){return d===null&&v&&(d=new XRWebGLBinding(r,t)),d},this.getFrame=function(){return x},this.getSession=function(){return r},this.setSession=async function(oe){if(r=oe,r!==null){if(P=e.getRenderTarget(),r.addEventListener("select",z),r.addEventListener("selectstart",z),r.addEventListener("selectend",z),r.addEventListener("squeeze",z),r.addEventListener("squeezestart",z),r.addEventListener("squeezeend",z),r.addEventListener("end",Y),r.addEventListener("inputsourceschange",ee),b.xrCompatible!==!0&&await t.makeXRCompatible(),g=e.getPixelRatio(),e.getSize(D),v&&"createProjectionLayer"in XRWebGLBinding.prototype){let be=null,Ve=null,Me=null;b.depth&&(Me=b.stencil?t.DEPTH24_STENCIL8:t.DEPTH_COMPONENT24,be=b.stencil?rr:vi,Ve=b.stencil?ps:ui);let Ge={colorFormat:t.RGBA8,depthFormat:Me,scaleFactor:s};d=this.getBinding(),u=d.createProjectionLayer(Ge),r.updateRenderState({layers:[u]}),e.setPixelRatio(1),e.setSize(u.textureWidth,u.textureHeight,!1),w=new Dn(u.textureWidth,u.textureHeight,{format:Bn,type:On,depthTexture:new Zi(u.textureWidth,u.textureHeight,Ve,void 0,void 0,void 0,void 0,void 0,void 0,be),stencilBuffer:b.stencil,colorSpace:e.outputColorSpace,samples:b.antialias?4:0,resolveDepthBuffer:u.ignoreDepthValues===!1,resolveStencilBuffer:u.ignoreDepthValues===!1,storeMultisampledDepthBuffer:u.ignoreDepthValues===!1,storeMultisampledStencilBuffer:u.ignoreDepthValues===!1})}else{let be={antialias:b.antialias,alpha:!0,depth:b.depth,stencil:b.stencil,framebufferScaleFactor:s};p=new XRWebGLLayer(r,t,be),r.updateRenderState({baseLayer:p}),e.setPixelRatio(1),e.setSize(p.framebufferWidth,p.framebufferHeight,!1),w=new Dn(p.framebufferWidth,p.framebufferHeight,{format:Bn,type:On,colorSpace:e.outputColorSpace,stencilBuffer:b.stencil,resolveDepthBuffer:p.ignoreDepthValues===!1,resolveStencilBuffer:p.ignoreDepthValues===!1,storeMultisampledDepthBuffer:p.ignoreDepthValues===!1,storeMultisampledStencilBuffer:p.ignoreDepthValues===!1})}w.isXRRenderTarget=!0,this.setFoveation(l),c=null,o=await r.requestReferenceSpace(a),He.setContext(r),He.start(),i.isPresenting=!0,i.dispatchEvent({type:"sessionstart"})}},this.getEnvironmentBlendMode=function(){if(r!==null)return r.environmentBlendMode},this.getDepthTexture=function(){return m.getDepthTexture()};function ee(oe){for(let le=0;le<oe.removed.length;le++){let be=oe.removed[le],Ve=E.indexOf(be);Ve>=0&&(E[Ve]=null,T[Ve].disconnect(be))}for(let le=0;le<oe.added.length;le++){let be=oe.added[le],Ve=E.indexOf(be);if(Ve===-1){for(let Ge=0;Ge<T.length;Ge++)if(Ge>=E.length){E.push(be),Ve=Ge;break}else if(E[Ge]===null){E[Ge]=be,Ve=Ge;break}if(Ve===-1)break}let Me=T[Ve];Me&&Me.connect(be)}}let J=new G,ie=new G;function ce(oe,le,be){J.setFromMatrixPosition(le.matrixWorld),ie.setFromMatrixPosition(be.matrixWorld);let Ve=J.distanceTo(ie),Me=le.projectionMatrix.elements,Ge=be.projectionMatrix.elements,Bt=Me[14]/(Me[10]-1),Je=Me[14]/(Me[10]+1),st=(Me[9]+1)/Me[5],mt=(Me[9]-1)/Me[5],tt=(Me[8]-1)/Me[0],Et=(Ge[8]+1)/Ge[0],Ht=Bt*tt,$t=Bt*Et,It=Ve/(-tt+Et),zt=It*-tt;if(le.matrixWorld.decompose(oe.position,oe.quaternion,oe.scale),oe.translateX(zt),oe.translateZ(It),oe.matrixWorld.compose(oe.position,oe.quaternion,oe.scale),oe.matrixWorldInverse.copy(oe.matrixWorld).invert(),Me[10]===-1)oe.projectionMatrix.copy(le.projectionMatrix),oe.projectionMatrixInverse.copy(le.projectionMatrixInverse);else{let W=Bt+It,nn=Je+It,ut=Ht-zt,F=$t+(Ve-zt),M=st*Je/nn*W,Z=mt*Je/nn*W;oe.projectionMatrix.makePerspective(ut,F,M,Z,W,nn),oe.projectionMatrixInverse.copy(oe.projectionMatrix).invert()}}function Se(oe,le){le===null?oe.matrixWorld.copy(oe.matrix):oe.matrixWorld.multiplyMatrices(le.matrixWorld,oe.matrix),oe.matrixWorldInverse.copy(oe.matrixWorld).invert()}this.updateCamera=function(oe){if(r===null)return;let le=oe.near,be=oe.far;m.texture!==null&&(m.depthNear>0&&(le=m.depthNear),m.depthFar>0&&(be=m.depthFar)),k.near=A.near=_.near=le,k.far=A.far=_.far=be,(N!==k.near||U!==k.far)&&(r.updateRenderState({depthNear:k.near,depthFar:k.far}),N=k.near,U=k.far),k.layers.mask=oe.layers.mask|6,_.layers.mask=k.layers.mask&-5,A.layers.mask=k.layers.mask&-3;let Ve=oe.parent,Me=k.cameras;Se(k,Ve);for(let Ge=0;Ge<Me.length;Ge++)Se(Me[Ge],Ve);Me.length===2?ce(k,_,A):k.projectionMatrix.copy(_.projectionMatrix),S===null&&oe.isPerspectiveCamera&&(S={camera:oe,fov:oe.fov,zoom:oe.zoom}),fe(oe,k,Ve)};function fe(oe,le,be){be===null?oe.matrix.copy(le.matrixWorld):(oe.matrix.copy(be.matrixWorld),oe.matrix.invert(),oe.matrix.multiply(le.matrixWorld)),oe.matrix.decompose(oe.position,oe.quaternion,oe.scale),oe.updateMatrixWorld(!0),oe.projectionMatrix.copy(le.projectionMatrix),oe.projectionMatrixInverse.copy(le.projectionMatrixInverse),oe.isPerspectiveCamera&&(oe.fov=ns*2*Math.atan(1/oe.projectionMatrix.elements[5]),oe.zoom=1)}this.getCamera=function(){return k},this.getFoveation=function(){if(!(u===null&&p===null))return l},this.setFoveation=function(oe){l=oe,u!==null&&(u.fixedFoveation=oe),p!==null&&p.fixedFoveation!==void 0&&(p.fixedFoveation=oe)},this.hasDepthSensing=function(){return m.texture!==null},this.getDepthSensingMesh=function(){return m.getMesh(k)},this.getCameraTexture=function(oe){return f[oe]};let Ie=null;function We(oe,le){if(h=le.getViewerPose(c||o),x=le,h!==null){let be=h.views;p!==null&&(e.setRenderTargetFramebuffer(w,p.framebuffer),e.setRenderTarget(w));let Ve=!1;be.length!==k.cameras.length&&(k.cameras.length=0,Ve=!0);for(let Je=0;Je<be.length;Je++){let st=be[Je],mt=null;if(p!==null)mt=p.getViewport(st);else{let Et=d.getViewSubImage(u,st);mt=Et.viewport,Je===0&&(e.setRenderTargetTextures(w,Et.colorTexture,Et.depthStencilTexture),e.setRenderTarget(w))}let tt=I[Je];tt===void 0&&(tt=new cn,tt.layers.enable(Je),tt.viewport=new Vt,I[Je]=tt),tt.matrix.fromArray(st.transform.matrix),tt.matrix.decompose(tt.position,tt.quaternion,tt.scale),tt.projectionMatrix.fromArray(st.projectionMatrix),tt.projectionMatrixInverse.copy(tt.projectionMatrix).invert(),tt.viewport.set(mt.x,mt.y,mt.width,mt.height),Je===0&&(k.matrix.copy(tt.matrix),k.matrix.decompose(k.position,k.quaternion,k.scale)),Ve===!0&&k.cameras.push(tt)}let Me=r.enabledFeatures;if(Me&&Me.includes("depth-sensing")&&r.depthUsage=="gpu-optimized"&&v){d=i.getBinding();let Je=d.getDepthInformation(be[0]);Je&&Je.isValid&&Je.texture&&m.init(Je,r.renderState)}if(Me&&Me.includes("camera-access")&&v){e.state.unbindTexture(),d=i.getBinding();for(let Je=0;Je<be.length;Je++){let st=be[Je].camera;if(st){let mt=f[st];mt||(mt=new Zs,f[st]=mt);let tt=d.getCameraImage(st);mt.sourceTexture=tt}}}}for(let be=0;be<T.length;be++){let Ve=E[be],Me=T[be];Ve!==null&&Me!==void 0&&Me.update(Ve,le,c||o)}Ie&&Ie(oe,le),le.detectedPlanes&&i.dispatchEvent({type:"planesdetected",data:le}),x=null}let He=new Ud;He.setAnimationLoop(We),this.setAnimationLoop=function(oe){Ie=oe},this.dispose=function(){}}},r1=new wt,Gd=new je;Gd.set(-1,0,0,0,1,0,0,0,1);function s1(n,e){function t(m,f){m.matrixAutoUpdate===!0&&m.updateMatrix(),f.value.copy(m.matrix)}function i(m,f){f.color.getRGB(m.fogColor.value,Qc(n)),f.isFog?(m.fogNear.value=f.near,m.fogFar.value=f.far):f.isFogExp2&&(m.fogDensity.value=f.density)}function r(m,f,b,P,w){f.isNodeMaterial?f.uniformsNeedUpdate=!1:f.isMeshBasicMaterial?s(m,f):f.isMeshLambertMaterial?(s(m,f),f.envMap&&(m.envMapIntensity.value=f.envMapIntensity)):f.isMeshToonMaterial?(s(m,f),d(m,f)):f.isMeshPhongMaterial?(s(m,f),h(m,f),f.envMap&&(m.envMapIntensity.value=f.envMapIntensity)):f.isMeshStandardMaterial?(s(m,f),u(m,f),f.isMeshPhysicalMaterial&&p(m,f,w)):f.isMeshMatcapMaterial?(s(m,f),x(m,f)):f.isMeshDepthMaterial?s(m,f):f.isMeshDistanceMaterial?(s(m,f),v(m,f)):f.isMeshNormalMaterial?s(m,f):f.isLineBasicMaterial?(o(m,f),f.isLineDashedMaterial&&a(m,f)):f.isPointsMaterial?l(m,f,b,P):f.isSpriteMaterial?c(m,f):f.isShadowMaterial?(m.color.value.copy(f.color),m.opacity.value=f.opacity):f.isShaderMaterial&&(f.uniformsNeedUpdate=!1)}function s(m,f){m.opacity.value=f.opacity,f.color&&m.diffuse.value.copy(f.color),f.emissive&&m.emissive.value.copy(f.emissive).multiplyScalar(f.emissiveIntensity),f.map&&(m.map.value=f.map,t(f.map,m.mapTransform)),f.alphaMap&&(m.alphaMap.value=f.alphaMap,t(f.alphaMap,m.alphaMapTransform)),f.bumpMap&&(m.bumpMap.value=f.bumpMap,t(f.bumpMap,m.bumpMapTransform),m.bumpScale.value=f.bumpScale,f.side===mn&&(m.bumpScale.value*=-1)),f.normalMap&&(m.normalMap.value=f.normalMap,t(f.normalMap,m.normalMapTransform),m.normalScale.value.copy(f.normalScale),f.side===mn&&m.normalScale.value.negate()),f.displacementMap&&(m.displacementMap.value=f.displacementMap,t(f.displacementMap,m.displacementMapTransform),m.displacementScale.value=f.displacementScale,m.displacementBias.value=f.displacementBias),f.emissiveMap&&(m.emissiveMap.value=f.emissiveMap,t(f.emissiveMap,m.emissiveMapTransform)),f.specularMap&&(m.specularMap.value=f.specularMap,t(f.specularMap,m.specularMapTransform)),f.alphaTest>0&&(m.alphaTest.value=f.alphaTest);let b=e.get(f),P=b.envMap,w=b.envMapRotation;P&&(m.envMap.value=P,m.envMapRotation.value.setFromMatrix4(r1.makeRotationFromEuler(w)).transpose(),P.isCubeTexture&&P.isRenderTargetTexture===!1&&m.envMapRotation.value.premultiply(Gd),m.reflectivity.value=f.reflectivity,m.ior.value=f.ior,m.refractionRatio.value=f.refractionRatio),f.lightMap&&(m.lightMap.value=f.lightMap,m.lightMapIntensity.value=f.lightMapIntensity,t(f.lightMap,m.lightMapTransform)),f.aoMap&&(m.aoMap.value=f.aoMap,m.aoMapIntensity.value=f.aoMapIntensity,t(f.aoMap,m.aoMapTransform))}function o(m,f){m.diffuse.value.copy(f.color),m.opacity.value=f.opacity,f.map&&(m.map.value=f.map,t(f.map,m.mapTransform))}function a(m,f){m.dashSize.value=f.dashSize,m.totalSize.value=f.dashSize+f.gapSize,m.scale.value=f.scale}function l(m,f,b,P){m.diffuse.value.copy(f.color),m.opacity.value=f.opacity,m.size.value=f.size*b,m.scale.value=P*.5,f.map&&(m.map.value=f.map,t(f.map,m.uvTransform)),f.alphaMap&&(m.alphaMap.value=f.alphaMap,t(f.alphaMap,m.alphaMapTransform)),f.alphaTest>0&&(m.alphaTest.value=f.alphaTest)}function c(m,f){m.diffuse.value.copy(f.color),m.opacity.value=f.opacity,m.rotation.value=f.rotation,f.map&&(m.map.value=f.map,t(f.map,m.mapTransform)),f.alphaMap&&(m.alphaMap.value=f.alphaMap,t(f.alphaMap,m.alphaMapTransform)),f.alphaTest>0&&(m.alphaTest.value=f.alphaTest)}function h(m,f){m.specular.value.copy(f.specular),m.shininess.value=Math.max(f.shininess,1e-4)}function d(m,f){f.gradientMap&&(m.gradientMap.value=f.gradientMap)}function u(m,f){m.metalness.value=f.metalness,f.metalnessMap&&(m.metalnessMap.value=f.metalnessMap,t(f.metalnessMap,m.metalnessMapTransform)),m.roughness.value=f.roughness,f.roughnessMap&&(m.roughnessMap.value=f.roughnessMap,t(f.roughnessMap,m.roughnessMapTransform)),f.envMap&&(m.envMapIntensity.value=f.envMapIntensity)}function p(m,f,b){m.ior.value=f.ior,f.sheen>0&&(m.sheenColor.value.copy(f.sheenColor).multiplyScalar(f.sheen),m.sheenRoughness.value=f.sheenRoughness,f.sheenColorMap&&(m.sheenColorMap.value=f.sheenColorMap,t(f.sheenColorMap,m.sheenColorMapTransform)),f.sheenRoughnessMap&&(m.sheenRoughnessMap.value=f.sheenRoughnessMap,t(f.sheenRoughnessMap,m.sheenRoughnessMapTransform))),f.clearcoat>0&&(m.clearcoat.value=f.clearcoat,m.clearcoatRoughness.value=f.clearcoatRoughness,f.clearcoatMap&&(m.clearcoatMap.value=f.clearcoatMap,t(f.clearcoatMap,m.clearcoatMapTransform)),f.clearcoatRoughnessMap&&(m.clearcoatRoughnessMap.value=f.clearcoatRoughnessMap,t(f.clearcoatRoughnessMap,m.clearcoatRoughnessMapTransform)),f.clearcoatNormalMap&&(m.clearcoatNormalMap.value=f.clearcoatNormalMap,t(f.clearcoatNormalMap,m.clearcoatNormalMapTransform),m.clearcoatNormalScale.value.copy(f.clearcoatNormalScale),f.side===mn&&m.clearcoatNormalScale.value.negate())),f.dispersion>0&&(m.dispersion.value=f.dispersion),f.retroreflectivity>0&&(m.retroreflectivity.value=f.retroreflectivity),f.iridescence>0&&(m.iridescence.value=f.iridescence,m.iridescenceIOR.value=f.iridescenceIOR,m.iridescenceThicknessMinimum.value=f.iridescenceThicknessRange[0],m.iridescenceThicknessMaximum.value=f.iridescenceThicknessRange[1],f.iridescenceMap&&(m.iridescenceMap.value=f.iridescenceMap,t(f.iridescenceMap,m.iridescenceMapTransform)),f.iridescenceThicknessMap&&(m.iridescenceThicknessMap.value=f.iridescenceThicknessMap,t(f.iridescenceThicknessMap,m.iridescenceThicknessMapTransform))),f.transmission>0&&(m.transmission.value=f.transmission,m.transmissionSamplerMap.value=b.texture,m.transmissionSamplerSize.value.set(b.width,b.height),f.transmissionMap&&(m.transmissionMap.value=f.transmissionMap,t(f.transmissionMap,m.transmissionMapTransform)),m.thickness.value=f.thickness,f.thicknessMap&&(m.thicknessMap.value=f.thicknessMap,t(f.thicknessMap,m.thicknessMapTransform)),m.attenuationDistance.value=f.attenuationDistance,m.attenuationColor.value.copy(f.attenuationColor)),f.anisotropy>0&&(m.anisotropyVector.value.set(f.anisotropy*Math.cos(f.anisotropyRotation),f.anisotropy*Math.sin(f.anisotropyRotation)),f.anisotropyMap&&(m.anisotropyMap.value=f.anisotropyMap,t(f.anisotropyMap,m.anisotropyMapTransform))),m.specularIntensity.value=f.specularIntensity,m.specularColor.value.copy(f.specularColor),f.specularColorMap&&(m.specularColorMap.value=f.specularColorMap,t(f.specularColorMap,m.specularColorMapTransform)),f.specularIntensityMap&&(m.specularIntensityMap.value=f.specularIntensityMap,t(f.specularIntensityMap,m.specularIntensityMapTransform))}function x(m,f){f.matcap&&(m.matcap.value=f.matcap)}function v(m,f){let b=e.get(f).light;m.referencePosition.value.setFromMatrixPosition(b.matrixWorld),m.nearDistance.value=b.shadow.camera.near,m.farDistance.value=b.shadow.camera.far}return{refreshFogUniforms:i,refreshMaterialUniforms:r}}function o1(n,e,t,i){let r={},s={},o=[],a=n.getParameter(n.MAX_UNIFORM_BUFFER_BINDINGS);function l(w,T){let E=T.program;i.uniformBlockBinding(w,E)}function c(w,T){let E=r[w.id];E===void 0&&(m(w),E=h(w),r[w.id]=E,w.addEventListener("dispose",b));let D=T.program;i.updateUBOMapping(w,D);let g=e.render.frame;s[w.id]!==g&&(u(w),s[w.id]=g)}function h(w){let T=d();w.__bindingPointIndex=T;let E=n.createBuffer(),D=w.__size,g=w.usage;return n.bindBuffer(n.UNIFORM_BUFFER,E),n.bufferData(n.UNIFORM_BUFFER,D,g),n.bindBuffer(n.UNIFORM_BUFFER,null),n.bindBufferBase(n.UNIFORM_BUFFER,T,E),E}function d(){for(let w=0;w<a;w++)if(o.indexOf(w)===-1)return o.push(w),w;return Ke("WebGLRenderer: Maximum number of simultaneously usable uniforms groups reached."),0}function u(w){let T=r[w.id],E=w.uniforms,D=w.__cache;n.bindBuffer(n.UNIFORM_BUFFER,T);for(let g=0,S=E.length;g<S;g++){let _=E[g];if(Array.isArray(_))for(let A=0,I=_.length;A<I;A++)p(_[A],g,A,D);else p(_,g,0,D)}n.bindBuffer(n.UNIFORM_BUFFER,null)}function p(w,T,E,D){if(v(w,T,E,D)===!0){let g=w.__offset,S=w.value;if(Array.isArray(S)){let _=0;for(let A=0;A<S.length;A++){let I=S[A],k=f(I);x(I,w.__data,_),typeof I!="number"&&typeof I!="boolean"&&!I.isMatrix3&&!ArrayBuffer.isView(I)&&(_+=k.storage/Float32Array.BYTES_PER_ELEMENT)}}else x(S,w.__data,0);n.bufferSubData(n.UNIFORM_BUFFER,g,w.__data)}}function x(w,T,E){typeof w=="number"||typeof w=="boolean"?T[0]=w:w.isMatrix3?(T[0]=w.elements[0],T[1]=w.elements[1],T[2]=w.elements[2],T[3]=0,T[4]=w.elements[3],T[5]=w.elements[4],T[6]=w.elements[5],T[7]=0,T[8]=w.elements[6],T[9]=w.elements[7],T[10]=w.elements[8],T[11]=0):ArrayBuffer.isView(w)?T.set(new w.constructor(w.buffer,w.byteOffset,T.length)):w.toArray(T,E)}function v(w,T,E,D){let g=w.value,S=T+"_"+E;if(D[S]===void 0)return typeof g=="number"||typeof g=="boolean"?D[S]=g:ArrayBuffer.isView(g)?D[S]=g.slice():D[S]=g.clone(),!0;{let _=D[S];if(typeof g=="number"||typeof g=="boolean"){if(_!==g)return D[S]=g,!0}else{if(ArrayBuffer.isView(g))return!0;if(_.equals(g)===!1)return _.copy(g),!0}}return!1}function m(w){let T=w.uniforms,E=0,D=16;for(let S=0,_=T.length;S<_;S++){let A=Array.isArray(T[S])?T[S]:[T[S]];for(let I=0,k=A.length;I<k;I++){let N=A[I],U=Array.isArray(N.value)?N.value:[N.value];for(let z=0,Y=U.length;z<Y;z++){let ee=U[z],J=f(ee),ie=E%D,ce=ie%J.boundary,Se=ie+ce;E+=ce,Se!==0&&D-Se<J.storage&&(E+=D-Se),N.__data=new Float32Array(J.storage/Float32Array.BYTES_PER_ELEMENT),N.__offset=E,E+=J.storage}}}let g=E%D;return g>0&&(E+=D-g),w.__size=E,w.__cache={},this}function f(w){let T={boundary:0,storage:0};return typeof w=="number"||typeof w=="boolean"?(T.boundary=4,T.storage=4):w.isVector2?(T.boundary=8,T.storage=8):w.isVector3||w.isColor?(T.boundary=16,T.storage=12):w.isVector4?(T.boundary=16,T.storage=16):w.isMatrix3?(T.boundary=48,T.storage=48):w.isMatrix4?(T.boundary=64,T.storage=64):w.isTexture?Ye("WebGLRenderer: Texture samplers can not be part of an uniforms group."):ArrayBuffer.isView(w)?(T.boundary=16,T.storage=w.byteLength):Ye("WebGLRenderer: Unsupported uniform value type.",w),T}function b(w){let T=w.target;T.removeEventListener("dispose",b);let E=o.indexOf(T.__bindingPointIndex);o.splice(E,1),n.deleteBuffer(r[T.id]),delete r[T.id],delete s[T.id]}function P(){for(let w in r)n.deleteBuffer(r[w]);o=[],r={},s={}}return{bind:l,update:c,dispose:P}}var a1=new Uint16Array([12469,15057,12620,14925,13266,14620,13807,14376,14323,13990,14545,13625,14713,13328,14840,12882,14931,12528,14996,12233,15039,11829,15066,11525,15080,11295,15085,10976,15082,10705,15073,10495,13880,14564,13898,14542,13977,14430,14158,14124,14393,13732,14556,13410,14702,12996,14814,12596,14891,12291,14937,11834,14957,11489,14958,11194,14943,10803,14921,10506,14893,10278,14858,9960,14484,14039,14487,14025,14499,13941,14524,13740,14574,13468,14654,13106,14743,12678,14818,12344,14867,11893,14889,11509,14893,11180,14881,10751,14852,10428,14812,10128,14765,9754,14712,9466,14764,13480,14764,13475,14766,13440,14766,13347,14769,13070,14786,12713,14816,12387,14844,11957,14860,11549,14868,11215,14855,10751,14825,10403,14782,10044,14729,9651,14666,9352,14599,9029,14967,12835,14966,12831,14963,12804,14954,12723,14936,12564,14917,12347,14900,11958,14886,11569,14878,11247,14859,10765,14828,10401,14784,10011,14727,9600,14660,9289,14586,8893,14508,8533,15111,12234,15110,12234,15104,12216,15092,12156,15067,12010,15028,11776,14981,11500,14942,11205,14902,10752,14861,10393,14812,9991,14752,9570,14682,9252,14603,8808,14519,8445,14431,8145,15209,11449,15208,11451,15202,11451,15190,11438,15163,11384,15117,11274,15055,10979,14994,10648,14932,10343,14871,9936,14803,9532,14729,9218,14645,8742,14556,8381,14461,8020,14365,7603,15273,10603,15272,10607,15267,10619,15256,10631,15231,10614,15182,10535,15118,10389,15042,10167,14963,9787,14883,9447,14800,9115,14710,8665,14615,8318,14514,7911,14411,7507,14279,7198,15314,9675,15313,9683,15309,9712,15298,9759,15277,9797,15229,9773,15166,9668,15084,9487,14995,9274,14898,8910,14800,8539,14697,8234,14590,7790,14479,7409,14367,7067,14178,6621,15337,8619,15337,8631,15333,8677,15325,8769,15305,8871,15264,8940,15202,8909,15119,8775,15022,8565,14916,8328,14804,8009,14688,7614,14569,7287,14448,6888,14321,6483,14088,6171,15350,7402,15350,7419,15347,7480,15340,7613,15322,7804,15287,7973,15229,8057,15148,8012,15046,7846,14933,7611,14810,7357,14682,7069,14552,6656,14421,6316,14251,5948,14007,5528,15356,5942,15356,5977,15353,6119,15348,6294,15332,6551,15302,6824,15249,7044,15171,7122,15070,7050,14949,6861,14818,6611,14679,6349,14538,6067,14398,5651,14189,5311,13935,4958,15359,4123,15359,4153,15356,4296,15353,4646,15338,5160,15311,5508,15263,5829,15188,6042,15088,6094,14966,6001,14826,5796,14678,5543,14527,5287,14377,4985,14133,4586,13869,4257,15360,1563,15360,1642,15358,2076,15354,2636,15341,3350,15317,4019,15273,4429,15203,4732,15105,4911,14981,4932,14836,4818,14679,4621,14517,4386,14359,4156,14083,3795,13808,3437,15360,122,15360,137,15358,285,15355,636,15344,1274,15322,2177,15281,2765,15215,3223,15120,3451,14995,3569,14846,3567,14681,3466,14511,3305,14344,3121,14037,2800,13753,2467,15360,0,15360,1,15359,21,15355,89,15346,253,15325,479,15287,796,15225,1148,15133,1492,15008,1749,14856,1882,14685,1886,14506,1783,14324,1608,13996,1398,13702,1183]),bi=null;function l1(){return bi===null&&(bi=new vr(a1,16,16,sr,hi),bi.name="DFG_LUT",bi.minFilter=vn,bi.magFilter=vn,bi.wrapS=xi,bi.wrapT=xi,bi.generateMipmaps=!1,bi.needsUpdate=!0),bi}var ys=class{constructor(e={}){let{canvas:t=ld(),context:i=null,depth:r=!0,stencil:s=!1,alpha:o=!1,antialias:a=!1,premultipliedAlpha:l=!0,preserveDrawingBuffer:c=!1,powerPreference:h="default",failIfMajorPerformanceCaveat:d=!1,reversedDepthBuffer:u=!1,outputBufferType:p=On}=e;this.isWebGLRenderer=!0;let x;if(i!==null){if(typeof WebGLRenderingContext<"u"&&i instanceof WebGLRenderingContext)throw new Error("THREE.WebGLRenderer: WebGL 1 is not supported since r163.");x=i.getContextAttributes().alpha}else x=o;let v=p,m=new Set([za,Ha,Ba]),f=new Set([On,ui,fs,ps,Ua,Fa]),b=new Uint32Array(4),P=new Int32Array(4),w=new G,T=null,E=null,D=[],g=[],S=null;this.domElement=t,this.debug={checkShaderErrors:!0,diagnostics:{keywords:!1},onShaderError:null},this.autoClear=!0,this.autoClearColor=!0,this.autoClearDepth=!0,this.autoClearStencil=!0,this.sortObjects=!0,this.clippingPlanes=[],this.localClippingEnabled=!1,this.toneMapping=ci,this.toneMappingExposure=1,this.transmissionResolutionScale=1;let _=this,A=!1,I=null,k=null,N=null,U=null;this._outputColorSpace=Pt;let z=0,Y=0,ee=null,J=-1,ie=null,ce=new Vt,Se=new Vt,fe=null,Ie=new ke(0),We=0,He=t.width,oe=t.height,le=1,be=null,Ve=null,Me=new Vt(0,0,He,oe),Ge=new Vt(0,0,He,oe),Bt=!1,Je=new os,st=!1,mt=!1,tt=new wt,Et=new G,Ht=new Vt,$t={background:null,fog:null,environment:null,overrideMaterial:null,isScene:!0},It=!1;function zt(){return ee===null?le:1}let W=i;function nn(R,V){return t.getContext(R,V)}let ut,F,M,Z,te,ae,ve,_e,C,L,O,H,q,K,re,pe,me,B,ue,se,ge,ye,de;try{let R={alpha:!0,depth:r,stencil:s,antialias:a,premultipliedAlpha:l,preserveDrawingBuffer:c,powerPreference:h,failIfMajorPerformanceCaveat:d};if("setAttribute"in t&&t.setAttribute("data-engine",`three.js r${"186"}`),t.addEventListener("webglcontextlost",vt,!1),t.addEventListener("webglcontextrestored",ot,!1),t.addEventListener("webglcontextcreationerror",Rt,!1),W===null){let V="webgl2";if(W=nn(V,R),W===null)throw nn(V)?new Error("THREE.WebGLRenderer: Error creating WebGL context with your selected attributes."):new Error("THREE.WebGLRenderer: Error creating WebGL context.")}Pe()}catch(R){throw t.removeEventListener("webglcontextlost",vt,!1),t.removeEventListener("webglcontextrestored",ot,!1),t.removeEventListener("webglcontextcreationerror",Rt,!1),Ke("WebGLRenderer: "+R.message),R}function Pe(){ut=new mx(W),ut.init(),ge=new t1(W,ut),F=new sx(W,ut,e,ge),M=new Qv(W,ut),F.reversedDepthBuffer&&u&&M.buffers.depth.setReversed(!0),k=W.createFramebuffer(),N=W.createFramebuffer(),U=W.createFramebuffer(),Z=new vx(W),te=new Bv,ae=new e1(W,ut,M,te,F,ge,Z),ve=new px(_),_e=new _m(W),ye=new ix(W,_e),C=new gx(W,_e,Z,ye),L=new _x(W,C,_e,ye,Z),B=new yx(W,F,ae),re=new ox(te),O=new Ov(_,ve,ut,F,ye,re),H=new s1(_,te),q=new zv,K=new $v(ut),me=new nx(_,ve,M,L,x,l),pe=new jv(_,L,F),de=new o1(W,Z,F,M),ue=new rx(W,ut,Z),se=new xx(W,ut,Z),Z.programs=O.programs,_.capabilities=F,_.extensions=ut,_.properties=te,_.renderLists=q,_.shadowMap=pe,_.state=M,_.info=Z}v!==On&&(S=new Mx(v,t.width,t.height,a,r,s));let Le=new xu(_,W);this.xr=Le,this.getContext=function(){return W},this.getContextAttributes=function(){return W.getContextAttributes()},this.forceContextLoss=function(){let R=ut.get("WEBGL_lose_context");R&&R.loseContext()},this.forceContextRestore=function(){let R=ut.get("WEBGL_lose_context");R&&R.restoreContext()},this.getPixelRatio=function(){return le},this.setPixelRatio=function(R){R!==void 0&&(le=R,this.setSize(He,oe,!1))},this.getSize=function(R){return R.set(He,oe)},this.setSize=function(R,V,ne=!0){if(Le.isPresenting){Ye("WebGLRenderer: Can't change size while VR device is presenting.");return}He=R,oe=V,t.width=Math.floor(R*le),t.height=Math.floor(V*le),ne===!0&&(t.style.width=R+"px",t.style.height=V+"px"),S!==null&&S.setSize(t.width,t.height),this.setViewport(0,0,R,V)},this.getDrawingBufferSize=function(R){return R.set(He*le,oe*le).floor()},this.setDrawingBufferSize=function(R,V,ne){He=R,oe=V,le=ne,t.width=Math.floor(R*ne),t.height=Math.floor(V*ne),this.setViewport(0,0,R,V)},this.setEffects=function(R){if(v===On){Ke("WebGLRenderer: setEffects() requires outputBufferType set to HalfFloatType or FloatType.");return}if(R){for(let V=0;V<R.length;V++)if(R[V].isOutputPass===!0){Ye("WebGLRenderer: OutputPass is not needed in setEffects(). Tone mapping and color space conversion are applied automatically.");break}}S.setEffects(R||[])},this.getCurrentViewport=function(R){return R.copy(ce)},this.getViewport=function(R){return R.copy(Me)},this.setViewport=function(R,V,ne,Q){R.isVector4?Me.set(R.x,R.y,R.z,R.w):Me.set(R,V,ne,Q),M.viewport(ce.copy(Me).multiplyScalar(le).round())},this.getScissor=function(R){return R.copy(Ge)},this.setScissor=function(R,V,ne,Q){R.isVector4?Ge.set(R.x,R.y,R.z,R.w):Ge.set(R,V,ne,Q),M.scissor(Se.copy(Ge).multiplyScalar(le).round())},this.getScissorTest=function(){return Bt},this.setScissorTest=function(R){M.setScissorTest(Bt=R)},this.setOpaqueSort=function(R){be=R},this.setTransparentSort=function(R){Ve=R},this.getClearColor=function(R){return R.copy(me.getClearColor())},this.setClearColor=function(){me.setClearColor(...arguments)},this.getClearAlpha=function(){return me.getClearAlpha()},this.setClearAlpha=function(){me.setClearAlpha(...arguments)},this.clear=function(R=!0,V=!0,ne=!0){let Q=0;if(R){let j=!1;if(ee!==null){let Ce=ee.texture.format;j=m.has(Ce)}if(j){let Ce=ee.texture.type,De=f.has(Ce),Ee=me.getClearColor(),Ae=me.getClearAlpha(),Ue=Ee.r,et=Ee.g,nt=Ee.b;De?(b[0]=Ue,b[1]=et,b[2]=nt,b[3]=Ae,W.clearBufferuiv(W.COLOR,0,b)):(P[0]=Ue,P[1]=et,P[2]=nt,P[3]=Ae,W.clearBufferiv(W.COLOR,0,P))}else Q|=W.COLOR_BUFFER_BIT}V&&(Q|=W.DEPTH_BUFFER_BIT,this.state.buffers.depth.setMask(!0)),ne&&(Q|=W.STENCIL_BUFFER_BIT,this.state.buffers.stencil.setMask(4294967295)),Q!==0&&W.clear(Q)},this.clearColor=function(){this.clear(!0,!1,!1)},this.clearDepth=function(){this.clear(!1,!0,!1)},this.clearStencil=function(){this.clear(!1,!1,!0)},this.setNodesHandler=function(R){R.setRenderer(this),I=R},this.dispose=function(){t.removeEventListener("webglcontextlost",vt,!1),t.removeEventListener("webglcontextrestored",ot,!1),t.removeEventListener("webglcontextcreationerror",Rt,!1),me.dispose(),q.dispose(),K.dispose(),te.dispose(),ve.dispose(),L.dispose(),ye.dispose(),de.dispose(),O.dispose(),Le.dispose(),Le.removeEventListener("sessionstart",X),Le.removeEventListener("sessionend",he),xe.stop()};function vt(R){R.preventDefault(),Kc("WebGLRenderer: Context Lost."),A=!0}function ot(){Kc("WebGLRenderer: Context Restored."),A=!1;let R=Z.autoReset,V=pe.enabled,ne=pe.autoUpdate,Q=pe.needsUpdate,j=pe.type;Pe(),Z.autoReset=R,pe.enabled=V,pe.autoUpdate=ne,pe.needsUpdate=Q,pe.type=j}function Rt(R){Ke("WebGLRenderer: A WebGL context could not be created. Reason: ",R.statusMessage)}function an(R){let V=R.target;V.removeEventListener("dispose",an),Gl(V)}function Gl(R){Vl(R),te.remove(R)}function Vl(R){let V=te.get(R).programs;V!==void 0&&(V.forEach(function(ne){O.releaseProgram(ne)}),R.isShaderMaterial&&O.releaseShaderCache(R))}this.renderBufferDirect=function(R,V,ne,Q,j,Ce){V===null&&(V=$t);let De=j.isMesh&&j.matrixWorld.determinantAffine()<0,Ee=kn(R,V,ne,Q,j);M.setMaterial(Q,De);let Ae=ne.index,Ue=1;if(Q.wireframe===!0){if(Ae=C.getWireframeAttribute(ne),Ae===void 0)return;Ue=2}let et=ne.drawRange,nt=ne.attributes.position,Fe=et.start*Ue,gt=(et.start+et.count)*Ue;Ce!==null&&(Fe=Math.max(Fe,Ce.start*Ue),gt=Math.min(gt,(Ce.start+Ce.count)*Ue)),Ae!==null?(Fe=Math.max(Fe,0),gt=Math.min(gt,Ae.count)):nt!=null&&(Fe=Math.max(Fe,0),gt=Math.min(gt,nt.count));let rn=gt-Fe;if(rn<0||rn===1/0)return;ye.setup(j,Q,Ee,ne,Ae);let Ut,kt=ue;if(Ae!==null&&(Ut=_e.get(Ae),kt=se,kt.setIndex(Ut)),j.isMesh)Q.wireframe===!0?(M.setLineWidth(Q.wireframeLinewidth*zt()),kt.setMode(W.LINES)):kt.setMode(W.TRIANGLES);else if(j.isLine){let _n=Q.linewidth;_n===void 0&&(_n=1),M.setLineWidth(_n*zt()),j.isLineSegments?kt.setMode(W.LINES):j.isLineLoop?kt.setMode(W.LINE_LOOP):kt.setMode(W.LINE_STRIP)}else j.isPoints?kt.setMode(W.POINTS):j.isSprite&&kt.setMode(W.TRIANGLES);if(j.isBatchedMesh)if(ut.get("WEBGL_multi_draw"))kt.renderMultiDraw(j._multiDrawStarts,j._multiDrawCounts,j._multiDrawCount);else{let _n=j._multiDrawStarts,Ne=j._multiDrawCounts,An=j._multiDrawCount,xt=Ae?_e.get(Ae).bytesPerElement:1,Zn=te.get(Q).currentProgram.getUniforms();for(let fi=0;fi<An;fi++)Zn.setValue(W,"_gl_DrawID",fi),kt.render(_n[fi]/xt,Ne[fi])}else if(j.isInstancedMesh)kt.renderInstances(Fe,rn,j.count);else if(ne.isInstancedBufferGeometry){let _n=ne._maxInstanceCount!==void 0?ne._maxInstanceCount:1/0,Ne=Math.min(ne.instanceCount,_n);kt.renderInstances(Fe,rn,Ne)}else kt.render(Fe,rn)};function Ro(R,V,ne,Q){I!==null&&R.isNodeMaterial&&I.setObject(Q,R),st===!0&&re.setState(R,ne,!1),R.transparent===!0&&R.side===Fn&&R.forceSinglePass===!1?(R.side=mn,R.needsUpdate=!0,Gt(R,V,Q),R.side=tr,R.needsUpdate=!0,Gt(R,V,Q),R.side=Fn):Gt(R,V,Q)}this.compile=function(R,V,ne=null){ne===null&&(ne=R),I!==null&&I.renderStart(R,V,ne),E=K.get(ne),E.init(V),g.push(E),ne.traverseVisible(function(j){j.isLight&&j.layers.test(V.layers)&&(E.pushLight(j),j.castShadow&&E.pushShadow(j))}),R!==ne&&R.traverseVisible(function(j){j.isLight&&j.layers.test(V.layers)&&(E.pushLight(j),j.castShadow&&E.pushShadow(j))}),E.setupLights(),I!==null&&I.updateLights(E.state.lightsArray),mt=this.localClippingEnabled,st=re.init(this.clippingPlanes,mt),st===!0&&re.setGlobalState(this.clippingPlanes,V),I!==null&&pe.render(E.state.shadowsArray,ne,V);let Q=new Set;return R.traverse(function(j){if(!(j.isMesh||j.isPoints||j.isLine||j.isSprite))return;let Ce=j.material;if(Ce)if(Array.isArray(Ce))for(let De=0;De<Ce.length;De++){let Ee=Ce[De];Ro(Ee,ne,V,j),Q.add(Ee)}else Ro(Ce,ne,V,j),Q.add(Ce)}),E=g.pop(),I!==null&&I.renderEnd(),Q},this.compileAsync=function(R,V,ne=null){let Q=this.compile(R,V,ne);return new Promise(j=>{function Ce(){if(Q.forEach(function(De){let Ae=te.get(De).currentProgram;(Ae===void 0||Ae.isReady())&&Q.delete(De)}),Q.size===0){j(R);return}setTimeout(Ce,10)}ut.get("KHR_parallel_shader_compile")!==null?Ce():setTimeout(Ce,10)})};let Es=null;function Wl(R){Es&&Es(R)}function X(){xe.stop()}function he(){xe.start()}let xe=new Ud;xe.setAnimationLoop(Wl),typeof self<"u"&&xe.setContext(self),this.setAnimationLoop=function(R){Es=R,Le.setAnimationLoop(R),R===null?xe.stop():xe.start()},Le.addEventListener("sessionstart",X),Le.addEventListener("sessionend",he),this.render=function(R,V){if(V!==void 0&&V.isCamera!==!0){Ke("WebGLRenderer.render: camera is not an instance of THREE.Camera.");return}if(A===!0)return;I!==null&&I.renderStart(R,V);let ne=Le.enabled===!0&&Le.isPresenting===!0,Q=S!==null&&(ee===null||ne)&&S.begin(_,ee);if(R.matrixWorldAutoUpdate===!0&&R.updateMatrixWorld(),V.parent===null&&V.matrixWorldAutoUpdate===!0&&V.updateMatrixWorld(),Le.enabled===!0&&Le.isPresenting===!0&&(S===null||S.isCompositing()===!1)&&(Le.cameraAutoUpdate===!0&&Le.updateCamera(V),V=Le.getCamera()),R.isScene===!0&&R.onBeforeRender(_,R,V,ee),E=K.get(R,g.length),E.init(V),E.state.textureUnits=ae.getTextureUnits(),g.push(E),tt.multiplyMatrices(V.projectionMatrix,V.matrixWorldInverse),Je.setFromProjectionMatrix(tt,ai,V.reversedDepth),mt=this.localClippingEnabled,st=re.init(this.clippingPlanes,mt),T=q.get(R,D.length),T.init(),D.push(T),Le.enabled===!0&&Le.isPresenting===!0){let De=_.xr.getDepthSensingMesh();De!==null&&$(De,V,-1/0,_.sortObjects)}$(R,V,0,_.sortObjects),T.finish(),I!==null&&I.updateLights(E.state.lightsArray),_.sortObjects===!0&&T.sort(be,Ve),It=Le.enabled===!1||Le.isPresenting===!1||Le.hasDepthSensing()===!1,It&&me.addToRenderList(T,R),this.info.render.frame++,this.info.autoReset===!0&&this.info.reset(),st===!0&&re.beginShadows();let j=E.state.shadowsArray;if(pe.render(j,R,V),st===!0&&re.endShadows(),(Q&&S.hasRenderPass())===!1){let De=T.opaque,Ee=T.transmissive;if(E.setupLights(),V.isArrayCamera){let Ae=V.cameras;if(Ee.length>0)for(let Ue=0,et=Ae.length;Ue<et;Ue++){let nt=Ae[Ue];we(De,Ee,R,nt)}It&&me.render(R);for(let Ue=0,et=Ae.length;Ue<et;Ue++){let nt=Ae[Ue];Re(T,R,nt,nt.viewport)}}else Ee.length>0&&we(De,Ee,R,V),It&&me.render(R),Re(T,R,V)}ee!==null&&Y===0&&(ae.updateMultisampleRenderTarget(ee),ae.updateRenderTargetMipmap(ee)),Q&&S.end(_),R.isScene===!0&&R.onAfterRender(_,R,V),ye.resetDefaultState(),J=-1,ie=null,g.pop(),g.length>0?(E=g[g.length-1],ae.setTextureUnits(E.state.textureUnits),st===!0&&re.setGlobalState(_.clippingPlanes,E.state.camera)):E=null,D.pop(),D.length>0?T=D[D.length-1]:T=null,I!==null&&I.renderEnd()};function $(R,V,ne,Q){if(R.visible===!1)return;if(R.layers.test(V.layers)){if(R.isGroup)ne=R.renderOrder;else if(R.isLOD)R.autoUpdate===!0&&R.update(V);else if(R.isLightProbeGrid)E.pushLightProbeGrid(R);else if(R.isLight)E.pushLight(R),R.castShadow&&E.pushShadow(R);else if(R.isSprite){if(!R.frustumCulled||R.intersectsFrustum(Je)){Q&&Ht.setFromMatrixPosition(R.matrixWorld).applyMatrix4(tt);let De=L.update(R),Ee=R.material;Ee.visible&&T.push(R,De,Ee,ne,Ht.z,null,V)}}else if((R.isMesh||R.isLine||R.isPoints)&&(!R.frustumCulled||R.intersectsFrustum(Je))){let De=L.update(R),Ee=R.material;if(Q&&(R.boundingSphere!==void 0?(R.boundingSphere===null&&R.computeBoundingSphere(),Ht.copy(R.boundingSphere.center)):(De.boundingSphere===null&&De.computeBoundingSphere(),Ht.copy(De.boundingSphere.center)),Ht.applyMatrix4(R.matrixWorld).applyMatrix4(tt)),Array.isArray(Ee)){let Ae=De.groups;for(let Ue=0,et=Ae.length;Ue<et;Ue++){let nt=Ae[Ue],Fe=Ee[nt.materialIndex];Fe&&Fe.visible&&T.push(R,De,Fe,ne,Ht.z,nt,V)}}else Ee.visible&&T.push(R,De,Ee,ne,Ht.z,null,V)}}let Ce=R.children;for(let De=0,Ee=Ce.length;De<Ee;De++)$(Ce[De],V,ne,Q)}function Re(R,V,ne,Q){let{opaque:j,transmissive:Ce,transparent:De}=R;E.setupLightsView(ne),st===!0&&re.setGlobalState(_.clippingPlanes,ne),Q&&M.viewport(ce.copy(Q)),j.length>0&&Qe(j,V,ne),Ce.length>0&&Qe(Ce,V,ne),De.length>0&&Qe(De,V,ne),M.buffers.depth.setTest(!0),M.buffers.depth.setMask(!0),M.buffers.color.setMask(!0),M.setPolygonOffset(!1)}function we(R,V,ne,Q){if((ne.isScene===!0?ne.overrideMaterial:null)!==null)return;if(E.state.transmissionRenderTarget[Q.id]===void 0){let Fe=ut.has("EXT_color_buffer_half_float")||ut.has("EXT_color_buffer_float");E.state.transmissionRenderTarget[Q.id]=new Dn(1,1,{generateMipmaps:!0,type:Fe?hi:On,minFilter:ir,samples:Math.max(4,F.samples),stencilBuffer:s,resolveDepthBuffer:!1,resolveStencilBuffer:!1,storeMultisampledDepthBuffer:!1,storeMultisampledStencilBuffer:!1,colorSpace:ht.workingColorSpace})}let Ce=E.state.transmissionRenderTarget[Q.id],De=Q.viewport||ce;Ce.setSize(De.z*_.transmissionResolutionScale,De.w*_.transmissionResolutionScale);let Ee=_.getRenderTarget(),Ae=_.getActiveCubeFace(),Ue=_.getActiveMipmapLevel();_.setRenderTarget(Ce),_.getClearColor(Ie),We=_.getClearAlpha(),We<1&&_.setClearColor(16777215,.5),_.clear(),It&&me.render(ne);let et=_.toneMapping;_.toneMapping=ci;let nt=Q.viewport;if(Q.viewport!==void 0&&(Q.viewport=void 0),E.setupLightsView(Q),st===!0&&re.setGlobalState(_.clippingPlanes,Q),Qe(R,ne,Q),ae.updateMultisampleRenderTarget(Ce),ae.updateRenderTargetMipmap(Ce),ut.has("WEBGL_multisampled_render_to_texture")===!1){let Fe=!1;for(let gt=0,rn=V.length;gt<rn;gt++){let Ut=V[gt],{object:kt,geometry:_n,material:Ne,group:An}=Ut;if(Ne.side===Fn&&kt.layers.test(Q.layers)){let xt=Ne.side;Ne.side=mn,Ne.needsUpdate=!0,qe(kt,ne,Q,_n,Ne,An),Ne.side=xt,Ne.needsUpdate=!0,Fe=!0}}Fe===!0&&(ae.updateMultisampleRenderTarget(Ce),ae.updateRenderTargetMipmap(Ce))}_.setRenderTarget(Ee,Ae,Ue),_.setClearColor(Ie,We),nt!==void 0&&(Q.viewport=nt),_.toneMapping=et}function Qe(R,V,ne){let Q=V.isScene===!0?V.overrideMaterial:null;for(let j=0,Ce=R.length;j<Ce;j++){let De=R[j],{object:Ee,geometry:Ae,group:Ue}=De,et=De.material;et.allowOverride===!0&&Q!==null&&(et=Q),Ee.layers.test(ne.layers)&&qe(Ee,V,ne,Ae,et,Ue)}}function qe(R,V,ne,Q,j,Ce){I!==null&&j.isNodeMaterial&&I.setObject(R,j),R.onBeforeRender(_,V,ne,Q,j,Ce),R.modelViewMatrix.multiplyMatrices(ne.matrixWorldInverse,R.matrixWorld),R.normalMatrix.getNormalMatrix(R.modelViewMatrix),j.onBeforeRender(_,V,ne,Q,R,Ce),j.transparent===!0&&j.side===Fn&&j.forceSinglePass===!1?(j.side=mn,j.needsUpdate=!0,_.renderBufferDirect(ne,V,Q,j,R,Ce),j.side=tr,j.needsUpdate=!0,_.renderBufferDirect(ne,V,Q,j,R,Ce),j.side=Fn):_.renderBufferDirect(ne,V,Q,j,R,Ce),R.onAfterRender(_,V,ne,Q,j,Ce)}function Gt(R,V,ne){V.isScene!==!0&&(V=$t);let Q=te.get(R),j=E.state.lights,Ce=E.state.shadowsArray,De=j.state.version,Ee=O.getParameters(R,j.state,Ce,V,ne,E.state.lightProbeGridArray),Ae=O.getProgramCacheKey(Ee),Ue=Q.programs;Q.environment=R.isMeshStandardMaterial||R.isMeshLambertMaterial||R.isMeshPhongMaterial?V.environment:null,Q.fog=V.fog;let et=R.isMeshStandardMaterial||R.isMeshLambertMaterial&&!R.envMap||R.isMeshPhongMaterial&&!R.envMap;Q.envMap=ve.get(R.envMap||Q.environment,et),Q.envMapRotation=Q.environment!==null&&R.envMap===null?V.environmentRotation:R.envMapRotation,Ue===void 0&&(R.addEventListener("dispose",an),Ue=new Map,Q.programs=Ue);let nt=Ue.get(Ae);if(nt!==void 0){if(Q.currentProgram===nt&&Q.lightsStateVersion===De)return Kt(R,Ee),nt}else Ee.uniforms=O.getUniforms(R),I!==null&&R.isNodeMaterial&&I.build(R,ne,Ee),R.onBeforeCompile(Ee,_),nt=O.acquireProgram(Ee,Ae),Ue.set(Ae,nt),Q.uniforms=Ee.uniforms;let Fe=Q.uniforms;return(!R.isShaderMaterial&&!R.isRawShaderMaterial||R.clipping===!0)&&(Fe.clippingPlanes=re.uniform),Kt(R,Ee),Q.needsLights=Co(R),Q.lightsStateVersion=De,Q.needsLights&&(Fe.ambientLightColor.value=j.state.ambient,Fe.lightProbe.value=j.state.probe,Fe.sunLights.value=j.state.sun,Fe.sunLightShadows.value=j.state.sunShadow,Fe.directionalLights.value=j.state.directional,Fe.directionalLightShadows.value=j.state.directionalShadow,Fe.spotLights.value=j.state.spot,Fe.spotLightShadows.value=j.state.spotShadow,Fe.rectAreaLights.value=j.state.rectArea,Fe.ltc_1.value=j.state.rectAreaLTC1,Fe.ltc_2.value=j.state.rectAreaLTC2,Fe.pointLights.value=j.state.point,Fe.pointLightShadows.value=j.state.pointShadow,Fe.hemisphereLights.value=j.state.hemi,Fe.sunShadowMatrix.value=j.state.sunShadowMatrix,Fe.sunShadowCascade.value=j.state.sunShadowCascade,Fe.directionalShadowMatrix.value=j.state.directionalShadowMatrix,Fe.spotLightMatrix.value=j.state.spotLightMatrix,Fe.spotLightMap.value=j.state.spotLightMap,Fe.pointShadowMatrix.value=j.state.pointShadowMatrix),Q.lightProbeGrid=E.state.lightProbeGridArray.length>0,Q.currentProgram=nt,Q.uniformsList=null,nt}function ct(R){if(R.uniformsList===null){let V=R.currentProgram.getUniforms();R.uniformsList=vs.seqWithValue(V.seq,R.uniforms)}return R.uniformsList}function Kt(R,V){let ne=te.get(R);ne.outputColorSpace=V.outputColorSpace,ne.batching=V.batching,ne.batchingColor=V.batchingColor,ne.instancing=V.instancing,ne.instancingColor=V.instancingColor,ne.instancingMorph=V.instancingMorph,ne.skinning=V.skinning,ne.morphTargets=V.morphTargets,ne.morphNormals=V.morphNormals,ne.morphColors=V.morphColors,ne.morphTargetsCount=V.morphTargetsCount,ne.numClippingPlanes=V.numClippingPlanes,ne.numIntersection=V.numClipIntersection,ne.vertexAlphas=V.vertexAlphas,ne.vertexTangents=V.vertexTangents,ne.toneMapping=V.toneMapping}function Yn(R,V){if(R.length===0)return null;if(R.length===1)return R[0].texture!==null?R[0]:null;w.setFromMatrixPosition(V.matrixWorld);for(let ne=0,Q=R.length;ne<Q;ne++){let j=R[ne];if(j.texture!==null&&j.boundingBox.containsPoint(w))return j}return null}function kn(R,V,ne,Q,j){V.isScene!==!0&&(V=$t),ae.resetTextureUnits();let Ce=V.fog,De=Q.isMeshStandardMaterial||Q.isMeshLambertMaterial||Q.isMeshPhongMaterial?V.environment:null,Ee=ee===null?_.outputColorSpace:ee.isXRRenderTarget===!0?ee.texture.colorSpace:ht.workingColorSpace,Ae=Q.isMeshStandardMaterial||Q.isMeshLambertMaterial&&!Q.envMap||Q.isMeshPhongMaterial&&!Q.envMap,Ue=ve.get(Q.envMap||De,Ae),et=Q.vertexColors===!0&&!!ne.attributes.color&&ne.attributes.color.itemSize===4,nt=!!ne.attributes.tangent&&(!!Q.normalMap||Q.anisotropy>0),Fe=!!ne.morphAttributes.position,gt=!!ne.morphAttributes.normal,rn=!!ne.morphAttributes.color,Ut=ci;Q.toneMapped&&(ee===null||ee.isXRRenderTarget===!0)&&(Ut=_.toneMapping);let kt=ne.morphAttributes.position||ne.morphAttributes.normal||ne.morphAttributes.color,_n=kt!==void 0?kt.length:0,Ne=te.get(Q),An=E.state.lights;if(st===!0&&(mt===!0||R!==ie)){let Dt=R===ie&&Q.id===J;re.setState(Q,R,Dt)}let xt=!1;Q.version===Ne.__version?(Ne.needsLights&&Ne.lightsStateVersion!==An.state.version||Ne.outputColorSpace!==Ee||j.isBatchedMesh&&Ne.batching===!1||!j.isBatchedMesh&&Ne.batching===!0||j.isBatchedMesh&&Ne.batchingColor===!0&&j._colorsTexture===null||j.isBatchedMesh&&Ne.batchingColor===!1&&j._colorsTexture!==null||j.isInstancedMesh&&Ne.instancing===!1||!j.isInstancedMesh&&Ne.instancing===!0||j.isSkinnedMesh&&Ne.skinning===!1||!j.isSkinnedMesh&&Ne.skinning===!0||j.isInstancedMesh&&Ne.instancingColor===!0&&j.instanceColor===null||j.isInstancedMesh&&Ne.instancingColor===!1&&j.instanceColor!==null||j.isInstancedMesh&&Ne.instancingMorph===!0&&j.morphTexture===null||j.isInstancedMesh&&Ne.instancingMorph===!1&&j.morphTexture!==null||Ne.envMap!==Ue||Q.fog===!0&&Ne.fog!==Ce||Ne.numClippingPlanes!==void 0&&(Ne.numClippingPlanes!==re.numPlanes||Ne.numIntersection!==re.numIntersection)||Ne.vertexAlphas!==et||Ne.vertexTangents!==nt||Ne.morphTargets!==Fe||Ne.morphNormals!==gt||Ne.morphColors!==rn||Ne.toneMapping!==Ut||Ne.morphTargetsCount!==_n||!!Ne.lightProbeGrid!=E.state.lightProbeGridArray.length>0)&&(xt=!0):(xt=!0,Ne.__version=Q.version);let Zn=Ne.currentProgram;xt===!0&&(Zn=Gt(Q,V,j),I&&Q.isNodeMaterial&&I.onUpdateProgram(Q,Zn,Ne));let fi=!1,Oi=!1,Dr=!1,Ct=Zn.getUniforms(),Jt=Ne.uniforms;if(M.useProgram(Zn.program)&&(fi=!0,Oi=!0,Dr=!0),Q.id!==J&&(J=Q.id,Oi=!0),Ne.needsLights){let Dt=Yn(E.state.lightProbeGridArray,j);Ne.lightProbeGrid!==Dt&&(Ne.lightProbeGrid=Dt,Oi=!0)}if(fi||ie!==R){M.buffers.depth.getReversed()&&R.reversedDepth!==!0&&(R._reversedDepth=!0,R.updateProjectionMatrix()),Ct.setValue(W,"projectionMatrix",R.projectionMatrix),Ct.setValue(W,"viewMatrix",R.matrixWorldInverse);let Hi=Ct.map.cameraPosition;Hi!==void 0&&Hi.setValue(W,Et.setFromMatrixPosition(R.matrixWorld)),F.logarithmicDepthBuffer&&Ct.setValue(W,"logDepthBufFC",2/(Math.log(R.far+1)/Math.LN2)),(Q.isMeshPhongMaterial||Q.isMeshToonMaterial||Q.isMeshLambertMaterial||Q.isMeshBasicMaterial||Q.isMeshStandardMaterial||Q.isShaderMaterial)&&Ct.setValue(W,"isOrthographic",R.isOrthographicCamera===!0),ie!==R&&(ie=R,Oi=!0,Dr=!0)}if(Ne.needsLights&&(An.state.sunShadowMap.length>0&&Ct.setValue(W,"sunShadowMap",An.state.sunShadowMap,ae),An.state.directionalShadowMap.length>0&&Ct.setValue(W,"directionalShadowMap",An.state.directionalShadowMap,ae),An.state.spotShadowMap.length>0&&Ct.setValue(W,"spotShadowMap",An.state.spotShadowMap,ae),An.state.pointShadowMap.length>0&&Ct.setValue(W,"pointShadowMap",An.state.pointShadowMap,ae)),j.isSkinnedMesh){Ct.setOptional(W,j,"bindMatrix"),Ct.setOptional(W,j,"bindMatrixInverse");let Dt=j.skeleton;Dt&&(Dt.boneTexture===null&&Dt.computeBoneTexture(),Ct.setValue(W,"boneTexture",Dt.boneTexture,ae))}j.isBatchedMesh&&(Ct.setOptional(W,j,"batchingTexture"),Ct.setValue(W,"batchingTexture",j._matricesTexture,ae),Ct.setOptional(W,j,"batchingIdTexture"),Ct.setValue(W,"batchingIdTexture",j._indirectTexture,ae),Ct.setOptional(W,j,"batchingColorTexture"),j._colorsTexture!==null&&Ct.setValue(W,"batchingColorTexture",j._colorsTexture,ae));let Bi=ne.morphAttributes;if((Bi.position!==void 0||Bi.normal!==void 0||Bi.color!==void 0)&&B.update(j,ne,Zn),(Oi||Ne.receiveShadow!==j.receiveShadow)&&(Ne.receiveShadow=j.receiveShadow,Ct.setValue(W,"receiveShadow",j.receiveShadow)),(Q.isMeshStandardMaterial||Q.isMeshLambertMaterial||Q.isMeshPhongMaterial)&&Q.envMap===null&&V.environment!==null&&(Jt.envMapIntensity.value=V.environmentIntensity),Jt.dfgLUT!==void 0&&(Jt.dfgLUT.value=l1()),Oi){if(Ct.setValue(W,"toneMappingExposure",_.toneMappingExposure),Ne.needsLights&&Xl(Jt,Dr),Ce&&Q.fog===!0&&H.refreshFogUniforms(Jt,Ce),H.refreshMaterialUniforms(Jt,Q,le,oe,E.state.transmissionRenderTarget[R.id]),Ne.needsLights&&Ne.lightProbeGrid){let Dt=Ne.lightProbeGrid;Jt.probesSH.value=Dt.texture,Jt.probesMin.value.copy(Dt.boundingBox.min),Jt.probesMax.value.copy(Dt.boundingBox.max),Jt.probesResolution.value.copy(Dt.resolution)}vs.upload(W,ct(Ne),Jt,ae)}if(Q.isShaderMaterial&&Q.uniformsNeedUpdate===!0&&(vs.upload(W,ct(Ne),Jt,ae),Q.uniformsNeedUpdate=!1),Q.isSpriteMaterial&&Ct.setValue(W,"center",j.center),Ct.setValue(W,"modelViewMatrix",j.modelViewMatrix),Ct.setValue(W,"normalMatrix",j.normalMatrix),Ct.setValue(W,"modelMatrix",j.matrixWorld),Q.uniformsGroups!==void 0){let Dt=Q.uniformsGroups;for(let Hi=0,Nr=Dt.length;Hi<Nr;Hi++){let Hu=Dt[Hi];de.update(Hu,Zn),de.bind(Hu,Zn)}}return Zn}function Xl(R,V){R.ambientLightColor.needsUpdate=V,R.lightProbe.needsUpdate=V,R.sunLights.needsUpdate=V,R.sunLightShadows.needsUpdate=V,R.directionalLights.needsUpdate=V,R.directionalLightShadows.needsUpdate=V,R.pointLights.needsUpdate=V,R.pointLightShadows.needsUpdate=V,R.spotLights.needsUpdate=V,R.spotLightShadows.needsUpdate=V,R.rectAreaLights.needsUpdate=V,R.hemisphereLights.needsUpdate=V}function Co(R){return R.isMeshLambertMaterial||R.isMeshToonMaterial||R.isMeshPhongMaterial||R.isMeshStandardMaterial||R.isShadowMaterial||R.isShaderMaterial&&R.lights===!0}this.getActiveCubeFace=function(){return z},this.getActiveMipmapLevel=function(){return Y},this.getRenderTarget=function(){return ee},this.setRenderTargetTextures=function(R,V,ne){let Q=te.get(R);Q.__autoAllocateDepthBuffer=R.resolveDepthBuffer===!1,Q.__autoAllocateDepthBuffer===!1&&(Q.__useRenderToTexture=!1),te.get(R.texture).__webglTexture=V,te.get(R.depthTexture).__webglTexture=Q.__autoAllocateDepthBuffer?void 0:ne,Q.__hasExternalTextures=!0},this.setRenderTargetFramebuffer=function(R,V){let ne=te.get(R);ne.__webglFramebuffer=V,ne.__useDefaultFramebuffer=V===void 0},this.setRenderTarget=function(R,V=0,ne=0){ee=R,z=V,Y=ne;let Q=null,j=!1,Ce=!1;if(R){let Ee=te.get(R);if(Ee.__useDefaultFramebuffer!==void 0){M.bindFramebuffer(W.FRAMEBUFFER,Ee.__webglFramebuffer),ce.copy(R.viewport),Se.copy(R.scissor),fe=R.scissorTest,M.viewport(ce),M.scissor(Se),M.setScissorTest(fe),J=-1;return}else if(Ee.__webglFramebuffer===void 0)ae.setupRenderTarget(R);else if(Ee.__hasExternalTextures)ae.rebindTextures(R,te.get(R.texture).__webglTexture,te.get(R.depthTexture).__webglTexture);else if(R.depthBuffer){let et=R.depthTexture;if(Ee.__boundDepthTexture!==et){if(et!==null&&te.has(et)&&(R.width!==et.image.width||R.height!==et.image.height))throw new Error("THREE.WebGLRenderer: Attached DepthTexture is initialized to the incorrect size.");ae.setupDepthRenderbuffer(R)}}let Ae=R.texture;(Ae.isData3DTexture||Ae.isDataArrayTexture||Ae.isCompressedArrayTexture)&&(Ce=!0);let Ue=te.get(R).__webglFramebuffer;R.isWebGLCubeRenderTarget?(Array.isArray(Ue[V])?Q=Ue[V][ne]:Q=Ue[V],j=!0):R.samples>0&&ae.useMultisampledRTT(R)===!1?Q=te.get(R).__webglMultisampledFramebuffer:Array.isArray(Ue)?Q=Ue[ne]:Q=Ue,ce.copy(R.viewport),Se.copy(R.scissor),fe=R.scissorTest}else ce.copy(Me).multiplyScalar(le).floor(),Se.copy(Ge).multiplyScalar(le).floor(),fe=Bt;if(ne!==0&&(Q=k),M.bindFramebuffer(W.FRAMEBUFFER,Q)&&M.drawBuffers(R,Q),M.viewport(ce),M.scissor(Se),M.setScissorTest(fe),j){let Ee=te.get(R.texture);W.framebufferTexture2D(W.FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_CUBE_MAP_POSITIVE_X+V,Ee.__webglTexture,ne)}else if(Ce){let Ee=V;for(let Ae=0;Ae<R.textures.length;Ae++){let Ue=te.get(R.textures[Ae]);W.framebufferTextureLayer(W.FRAMEBUFFER,W.COLOR_ATTACHMENT0+Ae,Ue.__webglTexture,ne,Ee)}}else if(R!==null&&ne!==0){let Ee=te.get(R.texture);W.framebufferTexture2D(W.FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_2D,Ee.__webglTexture,ne)}J=-1};function Lr(R){let V=te.get(R);return(V.__readFormat!==R.format||V.__readType!==R.type)&&(V.__readFormat=R.format,V.__readType=R.type,V.__formatReadable=F.textureFormatReadable(R.format),V.__typeReadable=F.textureTypeReadable(R.type)),V}this.readRenderTargetPixels=function(R,V,ne,Q,j,Ce,De,Ee=0){if(!(R&&R.isWebGLRenderTarget)){Ke("WebGLRenderer.readRenderTargetPixels: renderTarget is not THREE.WebGLRenderTarget.");return}let Ae=te.get(R).__webglFramebuffer;if(R.isWebGLCubeRenderTarget&&De!==void 0&&(Ae=Ae[De]),Ae){M.bindFramebuffer(W.FRAMEBUFFER,Ae);try{let Ue=R.textures[Ee],et=Ue.format,nt=Ue.type;R.textures.length>1&&W.readBuffer(W.COLOR_ATTACHMENT0+Ee);let Fe=Lr(Ue);if(Fe.__formatReadable===!1){Ke("WebGLRenderer.readRenderTargetPixels: renderTarget is not in RGBA or implementation defined format.");return}if(Fe.__typeReadable===!1){Ke("WebGLRenderer.readRenderTargetPixels: renderTarget is not in UnsignedByteType or implementation defined type.");return}V>=0&&V<=R.width-Q&&ne>=0&&ne<=R.height-j&&W.readPixels(V,ne,Q,j,ge.convert(et),ge.convert(nt),Ce)}finally{let Ue=ee!==null?te.get(ee).__webglFramebuffer:null;M.bindFramebuffer(W.FRAMEBUFFER,Ue)}}},this.readRenderTargetPixelsAsync=async function(R,V,ne,Q,j,Ce,De,Ee=0){if(!(R&&R.isWebGLRenderTarget))throw new Error("THREE.WebGLRenderer.readRenderTargetPixels: renderTarget is not THREE.WebGLRenderTarget.");let Ae=te.get(R).__webglFramebuffer;if(R.isWebGLCubeRenderTarget&&De!==void 0&&(Ae=Ae[De]),Ae)if(V>=0&&V<=R.width-Q&&ne>=0&&ne<=R.height-j){M.bindFramebuffer(W.FRAMEBUFFER,Ae);let Ue=R.textures[Ee],et=Ue.format,nt=Ue.type;R.textures.length>1&&W.readBuffer(W.COLOR_ATTACHMENT0+Ee);let Fe=Lr(Ue);if(Fe.__formatReadable===!1)throw new Error("THREE.WebGLRenderer.readRenderTargetPixelsAsync: renderTarget is not in RGBA or implementation defined format.");if(Fe.__typeReadable===!1)throw new Error("THREE.WebGLRenderer.readRenderTargetPixelsAsync: renderTarget is not in UnsignedByteType or implementation defined type.");let gt=W.createBuffer();W.bindBuffer(W.PIXEL_PACK_BUFFER,gt),W.bufferData(W.PIXEL_PACK_BUFFER,Ce.byteLength,W.STREAM_READ),W.readPixels(V,ne,Q,j,ge.convert(et),ge.convert(nt),0),W.bindBuffer(W.PIXEL_PACK_BUFFER,null);let rn=ee!==null?te.get(ee).__webglFramebuffer:null;M.bindFramebuffer(W.FRAMEBUFFER,rn);let Ut=W.fenceSync(W.SYNC_GPU_COMMANDS_COMPLETE,0);return W.flush(),await ud(W,Ut,4),W.bindBuffer(W.PIXEL_PACK_BUFFER,gt),W.getBufferSubData(W.PIXEL_PACK_BUFFER,0,Ce),W.bindBuffer(W.PIXEL_PACK_BUFFER,null),W.deleteBuffer(gt),W.deleteSync(Ut),Ce}else throw new Error("THREE.WebGLRenderer.readRenderTargetPixelsAsync: requested read bounds are out of range.")},this.copyFramebufferToTexture=function(R,V=null,ne=0){let Q=Math.pow(2,-ne),j=Math.floor(R.image.width*Q),Ce=Math.floor(R.image.height*Q),De=V!==null?V.x:0,Ee=V!==null?V.y:0;ae.setTexture2D(R,0),W.copyTexSubImage2D(W.TEXTURE_2D,ne,0,0,De,Ee,j,Ce),M.unbindTexture()},this.copyTextureToTexture=function(R,V,ne=null,Q=null,j=0,Ce=0){let De,Ee,Ae,Ue,et,nt,Fe,gt,rn,Ut=R.isCompressedTexture?R.mipmaps[Ce]:R.image;if(ne!==null)De=ne.max.x-ne.min.x,Ee=ne.max.y-ne.min.y,Ae=ne.isBox3?ne.max.z-ne.min.z:1,Ue=ne.min.x,et=ne.min.y,nt=ne.isBox3?ne.min.z:0;else{let Jt=Math.pow(2,-j);De=Math.floor(Ut.width*Jt),Ee=Math.floor(Ut.height*Jt),R.isDataArrayTexture?Ae=Ut.depth:R.isData3DTexture?Ae=Math.floor(Ut.depth*Jt):Ae=1,Ue=0,et=0,nt=0}Q!==null?(Fe=Q.x,gt=Q.y,rn=Q.z):(Fe=0,gt=0,rn=0);let kt=ge.convert(V.format),_n=ge.convert(V.type),Ne;V.isData3DTexture?(ae.setTexture3D(V,0),Ne=W.TEXTURE_3D):V.isDataArrayTexture||V.isCompressedArrayTexture?(ae.setTexture2DArray(V,0),Ne=W.TEXTURE_2D_ARRAY):(ae.setTexture2D(V,0),Ne=W.TEXTURE_2D),M.activeTexture(W.TEXTURE0),M.pixelStorei(W.UNPACK_FLIP_Y_WEBGL,V.flipY),M.pixelStorei(W.UNPACK_PREMULTIPLY_ALPHA_WEBGL,V.premultiplyAlpha),M.pixelStorei(W.UNPACK_ALIGNMENT,V.unpackAlignment);let An=M.getParameter(W.UNPACK_ROW_LENGTH),xt=M.getParameter(W.UNPACK_IMAGE_HEIGHT),Zn=M.getParameter(W.UNPACK_SKIP_PIXELS),fi=M.getParameter(W.UNPACK_SKIP_ROWS),Oi=M.getParameter(W.UNPACK_SKIP_IMAGES);M.pixelStorei(W.UNPACK_ROW_LENGTH,Ut.width),M.pixelStorei(W.UNPACK_IMAGE_HEIGHT,Ut.height),M.pixelStorei(W.UNPACK_SKIP_PIXELS,Ue),M.pixelStorei(W.UNPACK_SKIP_ROWS,et),M.pixelStorei(W.UNPACK_SKIP_IMAGES,nt);let Dr=R.isDataArrayTexture||R.isData3DTexture,Ct=V.isDataArrayTexture||V.isData3DTexture;if(R.isDepthTexture){let Jt=te.get(R),Bi=te.get(V),Dt=te.get(Jt.__renderTarget),Hi=te.get(Bi.__renderTarget);M.bindFramebuffer(W.READ_FRAMEBUFFER,Dt.__webglFramebuffer),M.bindFramebuffer(W.DRAW_FRAMEBUFFER,Hi.__webglFramebuffer);for(let Nr=0;Nr<Ae;Nr++)Dr&&(W.framebufferTextureLayer(W.READ_FRAMEBUFFER,W.COLOR_ATTACHMENT0,te.get(R).__webglTexture,j,nt+Nr),W.framebufferTextureLayer(W.DRAW_FRAMEBUFFER,W.COLOR_ATTACHMENT0,te.get(V).__webglTexture,Ce,rn+Nr)),W.blitFramebuffer(Ue,et,De,Ee,Fe,gt,De,Ee,W.DEPTH_BUFFER_BIT,W.NEAREST);M.bindFramebuffer(W.READ_FRAMEBUFFER,null),M.bindFramebuffer(W.DRAW_FRAMEBUFFER,null)}else if(j!==0||R.isRenderTargetTexture||te.has(R)){let Jt=te.get(R),Bi=te.get(V);M.bindFramebuffer(W.READ_FRAMEBUFFER,N),M.bindFramebuffer(W.DRAW_FRAMEBUFFER,U);for(let Dt=0;Dt<Ae;Dt++)Dr?W.framebufferTextureLayer(W.READ_FRAMEBUFFER,W.COLOR_ATTACHMENT0,Jt.__webglTexture,j,nt+Dt):W.framebufferTexture2D(W.READ_FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_2D,Jt.__webglTexture,j),Ct?W.framebufferTextureLayer(W.DRAW_FRAMEBUFFER,W.COLOR_ATTACHMENT0,Bi.__webglTexture,Ce,rn+Dt):W.framebufferTexture2D(W.DRAW_FRAMEBUFFER,W.COLOR_ATTACHMENT0,W.TEXTURE_2D,Bi.__webglTexture,Ce),j!==0?W.blitFramebuffer(Ue,et,De,Ee,Fe,gt,De,Ee,W.COLOR_BUFFER_BIT,W.NEAREST):Ct?W.copyTexSubImage3D(Ne,Ce,Fe,gt,rn+Dt,Ue,et,De,Ee):W.copyTexSubImage2D(Ne,Ce,Fe,gt,Ue,et,De,Ee);M.bindFramebuffer(W.READ_FRAMEBUFFER,null),M.bindFramebuffer(W.DRAW_FRAMEBUFFER,null)}else Ct?R.isDataTexture||R.isData3DTexture?W.texSubImage3D(Ne,Ce,Fe,gt,rn,De,Ee,Ae,kt,_n,Ut.data):V.isCompressedArrayTexture?W.compressedTexSubImage3D(Ne,Ce,Fe,gt,rn,De,Ee,Ae,kt,Ut.data):W.texSubImage3D(Ne,Ce,Fe,gt,rn,De,Ee,Ae,kt,_n,Ut):R.isDataTexture?W.texSubImage2D(W.TEXTURE_2D,Ce,Fe,gt,De,Ee,kt,_n,Ut.data):R.isCompressedTexture?W.compressedTexSubImage2D(W.TEXTURE_2D,Ce,Fe,gt,Ut.width,Ut.height,kt,Ut.data):W.texSubImage2D(W.TEXTURE_2D,Ce,Fe,gt,De,Ee,kt,_n,Ut);M.pixelStorei(W.UNPACK_ROW_LENGTH,An),M.pixelStorei(W.UNPACK_IMAGE_HEIGHT,xt),M.pixelStorei(W.UNPACK_SKIP_PIXELS,Zn),M.pixelStorei(W.UNPACK_SKIP_ROWS,fi),M.pixelStorei(W.UNPACK_SKIP_IMAGES,Oi),Ce===0&&V.generateMipmaps&&W.generateMipmap(Ne),M.unbindTexture()},this.initRenderTarget=function(R){te.get(R).__webglFramebuffer===void 0&&ae.setupRenderTarget(R)},this.initTexture=function(R){R.isCubeTexture?ae.setTextureCube(R,0):R.isData3DTexture?ae.setTexture3D(R,0):R.isDataArrayTexture||R.isCompressedArrayTexture?ae.setTexture2DArray(R,0):ae.setTexture2D(R,0),M.unbindTexture()},this.resetState=function(){z=0,Y=0,ee=null,M.reset(),ye.reset()},typeof __THREE_DEVTOOLS__<"u"&&__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("observe",{detail:this}))}get coordinateSystem(){return ai}get outputColorSpace(){return this._outputColorSpace}set outputColorSpace(e){this._outputColorSpace=e;let t=this.getContext();t.drawingBufferColorSpace=ht._getDrawingBufferColorSpace(e),t.unpackColorSpace=ht._getUnpackColorSpace()}};var Vd={type:"change"},yu={type:"start"},Xd={type:"end"},Rl=new Yi,Wd=new Wn,c1=Math.cos(70*jc.DEG2RAD),fn=new G,Hn=2*Math.PI,Tt={NONE:-1,ROTATE:0,DOLLY:1,PAN:2,TOUCH_ROTATE:3,TOUCH_PAN:4,TOUCH_DOLLY_PAN:5,TOUCH_DOLLY_ROTATE:6},vu=1e-6,Cl=class extends ro{constructor(e,t=null){super(e,t),this.state=Tt.NONE,this.target=new G,this.cursor=new G,this.minDistance=0,this.maxDistance=1/0,this.minZoom=0,this.maxZoom=1/0,this.minTargetRadius=0,this.maxTargetRadius=1/0,this.minPolarAngle=0,this.maxPolarAngle=Math.PI,this.minAzimuthAngle=-1/0,this.maxAzimuthAngle=1/0,this.enableDamping=!1,this.dampingFactor=.05,this.enableZoom=!0,this.zoomSpeed=1,this.enableRotate=!0,this.rotateSpeed=1,this.keyRotateSpeed=1,this.enablePan=!0,this.panSpeed=1,this.screenSpacePanning=!0,this.keyPanSpeed=7,this.zoomToCursor=!1,this.autoRotate=!1,this.autoRotateSpeed=2,this.keys={LEFT:"ArrowLeft",UP:"ArrowUp",RIGHT:"ArrowRight",BOTTOM:"ArrowDown"},this.mouseButtons={LEFT:Qi.ROTATE,MIDDLE:Qi.DOLLY,RIGHT:Qi.PAN},this.touches={ONE:er.ROTATE,TWO:er.DOLLY_PAN},this.target0=this.target.clone(),this.position0=this.object.position.clone(),this.zoom0=this.object.zoom,this._cursorStyle="auto",this._domElementKeyEvents=null,this._lastPosition=new G,this._lastQuaternion=new un,this._lastTargetPosition=new G,this._quat=new un().setFromUnitVectors(e.up,new G(0,1,0)),this._quatInverse=this._quat.clone().invert(),this._spherical=new us,this._sphericalDelta=new us,this._scale=1,this._panOffset=new G,this._rotateStart=new Xe,this._rotateEnd=new Xe,this._rotateDelta=new Xe,this._panStart=new Xe,this._panEnd=new Xe,this._panDelta=new Xe,this._dollyStart=new Xe,this._dollyEnd=new Xe,this._dollyDelta=new Xe,this._dollyDirection=new G,this._mouse=new Xe,this._performCursorZoom=!1,this._pointers=[],this._pointerPositions={},this._controlActive=!1,this._onPointerMove=h1.bind(this),this._onPointerDown=u1.bind(this),this._onPointerUp=d1.bind(this),this._onContextMenu=y1.bind(this),this._onMouseWheel=m1.bind(this),this._onKeyDown=g1.bind(this),this._onTouchStart=x1.bind(this),this._onTouchMove=v1.bind(this),this._onMouseDown=f1.bind(this),this._onMouseMove=p1.bind(this),this._interceptControlDown=_1.bind(this),this._interceptControlUp=b1.bind(this),this.domElement!==null&&this.connect(this.domElement),this.update()}set cursorStyle(e){this._cursorStyle=e,e==="grab"?this.domElement.style.cursor="grab":this.domElement.style.cursor="auto"}get cursorStyle(){return this._cursorStyle}connect(e){super.connect(e),this.domElement.addEventListener("pointerdown",this._onPointerDown),this.domElement.addEventListener("pointercancel",this._onPointerUp),this.domElement.addEventListener("contextmenu",this._onContextMenu),this.domElement.addEventListener("wheel",this._onMouseWheel,{passive:!1}),this.domElement.getRootNode().addEventListener("keydown",this._interceptControlDown,{passive:!0,capture:!0}),this.domElement.style.touchAction="none"}disconnect(){this.state=Tt.NONE,this.domElement.removeEventListener("pointerdown",this._onPointerDown),this.domElement.ownerDocument.removeEventListener("pointermove",this._onPointerMove),this.domElement.ownerDocument.removeEventListener("pointerup",this._onPointerUp),this.domElement.removeEventListener("pointercancel",this._onPointerUp),this.domElement.removeEventListener("wheel",this._onMouseWheel),this.domElement.removeEventListener("contextmenu",this._onContextMenu),this.stopListenToKeyEvents();let e=this.domElement.getRootNode();e.removeEventListener("keydown",this._interceptControlDown,{capture:!0}),e.removeEventListener("keyup",this._interceptControlUp,{capture:!0}),this._controlActive=!1,this._pointers.length=0,this._pointerPositions={},this.domElement.style.touchAction="",this.domElement.style.cursor="auto"}dispose(){this.disconnect()}getPolarAngle(){return this._spherical.phi}getAzimuthalAngle(){return this._spherical.theta}getDistance(){return this.object.position.distanceTo(this.target)}listenToKeyEvents(e){e.addEventListener("keydown",this._onKeyDown),this._domElementKeyEvents=e}stopListenToKeyEvents(){this._domElementKeyEvents!==null&&(this._domElementKeyEvents.removeEventListener("keydown",this._onKeyDown),this._domElementKeyEvents=null)}saveState(){this.target0.copy(this.target),this.position0.copy(this.object.position),this.zoom0=this.object.zoom}reset(){this.target.copy(this.target0),this.object.position.copy(this.position0),this.object.zoom=this.zoom0,this.object.updateProjectionMatrix(),this.dispatchEvent(Vd),this.update(),this.state=Tt.NONE}pan(e,t){this._pan(e,t),this.update()}dollyIn(e){this._dollyIn(e),this.update()}dollyOut(e){this._dollyOut(e),this.update()}rotateLeft(e){this._rotateLeft(e),this.update()}rotateUp(e){this._rotateUp(e),this.update()}update(e=null){let t=this.object.position;fn.copy(t).sub(this.target),fn.applyQuaternion(this._quat),this._spherical.setFromVector3(fn),this.autoRotate&&this.state===Tt.NONE&&this._rotateLeft(this._getAutoRotationAngle(e)),this.enableDamping?(this._spherical.theta+=this._sphericalDelta.theta*this.dampingFactor,this._spherical.phi+=this._sphericalDelta.phi*this.dampingFactor):(this._spherical.theta+=this._sphericalDelta.theta,this._spherical.phi+=this._sphericalDelta.phi);let i=this.minAzimuthAngle,r=this.maxAzimuthAngle;isFinite(i)&&isFinite(r)&&(i<-Math.PI?i+=Hn:i>Math.PI&&(i-=Hn),r<-Math.PI?r+=Hn:r>Math.PI&&(r-=Hn),i<=r?this._spherical.theta=Math.max(i,Math.min(r,this._spherical.theta)):this._spherical.theta=this._spherical.theta>(i+r)/2?Math.max(i,this._spherical.theta):Math.min(r,this._spherical.theta)),this._spherical.phi=Math.max(this.minPolarAngle,Math.min(this.maxPolarAngle,this._spherical.phi)),this._spherical.makeSafe(),this.enableDamping===!0?this.target.addScaledVector(this._panOffset,this.dampingFactor):this.target.add(this._panOffset),this.target.sub(this.cursor),this.target.clampLength(this.minTargetRadius,this.maxTargetRadius),this.target.add(this.cursor);let s=!1;if(this.zoomToCursor&&this._performCursorZoom||this.object.isOrthographicCamera)this._spherical.radius=this._clampDistance(this._spherical.radius);else{let o=this._spherical.radius;this._spherical.radius=this._clampDistance(this._spherical.radius*this._scale),s=o!=this._spherical.radius}if(fn.setFromSpherical(this._spherical),fn.applyQuaternion(this._quatInverse),t.copy(this.target).add(fn),this.object.lookAt(this.target),this.enableDamping===!0?(this._sphericalDelta.theta*=1-this.dampingFactor,this._sphericalDelta.phi*=1-this.dampingFactor,this._panOffset.multiplyScalar(1-this.dampingFactor)):(this._sphericalDelta.set(0,0,0),this._panOffset.set(0,0,0)),this.zoomToCursor&&this._performCursorZoom){let o=null;if(this.object.isPerspectiveCamera){let a=fn.length();o=this._clampDistance(a*this._scale);let l=a-o;this.object.position.addScaledVector(this._dollyDirection,l),this.object.updateMatrixWorld(),s=!!l}else if(this.object.isOrthographicCamera){let a=new G(this._mouse.x,this._mouse.y,0);a.unproject(this.object);let l=this.object.zoom;this.object.zoom=Math.max(this.minZoom,Math.min(this.maxZoom,this.object.zoom/this._scale)),this.object.updateProjectionMatrix(),s=l!==this.object.zoom;let c=new G(this._mouse.x,this._mouse.y,0);c.unproject(this.object),this.object.position.sub(c).add(a),this.object.updateMatrixWorld(),o=fn.length()}else console.warn("WARNING: OrbitControls.js encountered an unknown camera type - zoom to cursor disabled."),this.zoomToCursor=!1;o!==null&&(this.screenSpacePanning?this.target.set(0,0,-1).transformDirection(this.object.matrix).multiplyScalar(o).add(this.object.position):(Rl.origin.copy(this.object.position),Rl.direction.set(0,0,-1).transformDirection(this.object.matrix),Math.abs(this.object.up.dot(Rl.direction))<c1?this.object.lookAt(this.target):(Wd.setFromNormalAndCoplanarPoint(this.object.up,this.target),Rl.intersectPlane(Wd,this.target))))}else if(this.object.isOrthographicCamera){let o=this.object.zoom;this.object.zoom=Math.max(this.minZoom,Math.min(this.maxZoom,this.object.zoom/this._scale)),o!==this.object.zoom&&(this.object.updateProjectionMatrix(),s=!0)}return this._scale=1,this._performCursorZoom=!1,s||this._lastPosition.distanceToSquared(this.object.position)>vu||8*(1-this._lastQuaternion.dot(this.object.quaternion))>vu||this._lastTargetPosition.distanceToSquared(this.target)>vu?(this.dispatchEvent(Vd),this._lastPosition.copy(this.object.position),this._lastQuaternion.copy(this.object.quaternion),this._lastTargetPosition.copy(this.target),!0):!1}_getAutoRotationAngle(e){return e!==null?Hn/60*this.autoRotateSpeed*e:Hn/60/60*this.autoRotateSpeed}_getZoomScale(e){let t=Math.abs(e*.01);return Math.pow(.95,this.zoomSpeed*t)}_rotateLeft(e){this._sphericalDelta.theta-=e}_rotateUp(e){this._sphericalDelta.phi-=e}_panLeft(e,t){fn.setFromMatrixColumn(t,0),fn.multiplyScalar(-e),this._panOffset.add(fn)}_panUp(e,t){this.screenSpacePanning===!0?fn.setFromMatrixColumn(t,1):(fn.setFromMatrixColumn(t,0),fn.crossVectors(this.object.up,fn)),fn.multiplyScalar(e),this._panOffset.add(fn)}_pan(e,t){let i=this.domElement;if(this.object.isPerspectiveCamera){let r=this.object.position;fn.copy(r).sub(this.target);let s=fn.length();s*=Math.tan(this.object.fov/2*Math.PI/180),this._panLeft(2*e*s/i.clientHeight,this.object.matrix),this._panUp(2*t*s/i.clientHeight,this.object.matrix)}else this.object.isOrthographicCamera?(this._panLeft(e*(this.object.right-this.object.left)/this.object.zoom/i.clientWidth,this.object.matrix),this._panUp(t*(this.object.top-this.object.bottom)/this.object.zoom/i.clientHeight,this.object.matrix)):(console.warn("WARNING: OrbitControls.js encountered an unknown camera type - pan disabled."),this.enablePan=!1)}_dollyOut(e){this.object.isPerspectiveCamera||this.object.isOrthographicCamera?this._scale/=e:(console.warn("WARNING: OrbitControls.js encountered an unknown camera type - dolly/zoom disabled."),this.enableZoom=!1)}_dollyIn(e){this.object.isPerspectiveCamera||this.object.isOrthographicCamera?this._scale*=e:(console.warn("WARNING: OrbitControls.js encountered an unknown camera type - dolly/zoom disabled."),this.enableZoom=!1)}_updateZoomParameters(e,t){if(!this.zoomToCursor)return;this._performCursorZoom=!0;let i=this.domElement.getBoundingClientRect(),r=e-i.left,s=t-i.top,o=i.width,a=i.height;this._mouse.x=r/o*2-1,this._mouse.y=-(s/a)*2+1,this._dollyDirection.set(this._mouse.x,this._mouse.y,1).unproject(this.object).sub(this.object.position).normalize()}_clampDistance(e){return Math.max(this.minDistance,Math.min(this.maxDistance,e))}_handleMouseDownRotate(e){this._rotateStart.set(e.clientX,e.clientY)}_handleMouseDownDolly(e){this._updateZoomParameters(e.clientX,e.clientX),this._dollyStart.set(e.clientX,e.clientY)}_handleMouseDownPan(e){this._panStart.set(e.clientX,e.clientY)}_handleMouseMoveRotate(e){this._rotateEnd.set(e.clientX,e.clientY),this._rotateDelta.subVectors(this._rotateEnd,this._rotateStart).multiplyScalar(this.rotateSpeed);let t=this.domElement;this._rotateLeft(Hn*this._rotateDelta.x/t.clientHeight),this._rotateUp(Hn*this._rotateDelta.y/t.clientHeight),this._rotateStart.copy(this._rotateEnd),this.update()}_handleMouseMoveDolly(e){this._dollyEnd.set(e.clientX,e.clientY),this._dollyDelta.subVectors(this._dollyEnd,this._dollyStart),this._dollyDelta.y>0?this._dollyOut(this._getZoomScale(this._dollyDelta.y)):this._dollyDelta.y<0&&this._dollyIn(this._getZoomScale(this._dollyDelta.y)),this._dollyStart.copy(this._dollyEnd),this.update()}_handleMouseMovePan(e){this._panEnd.set(e.clientX,e.clientY),this._panDelta.subVectors(this._panEnd,this._panStart).multiplyScalar(this.panSpeed),this._pan(this._panDelta.x,this._panDelta.y),this._panStart.copy(this._panEnd),this.update()}_handleMouseWheel(e){this._updateZoomParameters(e.clientX,e.clientY),e.deltaY<0?this._dollyIn(this._getZoomScale(e.deltaY)):e.deltaY>0&&this._dollyOut(this._getZoomScale(e.deltaY)),this.update()}_handleKeyDown(e){let t=!1;switch(e.code){case this.keys.UP:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateUp(Hn*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(0,this.keyPanSpeed),t=!0;break;case this.keys.BOTTOM:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateUp(-Hn*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(0,-this.keyPanSpeed),t=!0;break;case this.keys.LEFT:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateLeft(Hn*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(this.keyPanSpeed,0),t=!0;break;case this.keys.RIGHT:e.ctrlKey||e.metaKey||e.shiftKey?this.enableRotate&&this._rotateLeft(-Hn*this.keyRotateSpeed/this.domElement.clientHeight):this.enablePan&&this._pan(-this.keyPanSpeed,0),t=!0;break}t&&(e.preventDefault(),this.update())}_handleTouchStartRotate(e){if(this._pointers.length===1)this._rotateStart.set(e.pageX,e.pageY);else{let t=this._getSecondPointerPosition(e),i=.5*(e.pageX+t.x),r=.5*(e.pageY+t.y);this._rotateStart.set(i,r)}}_handleTouchStartPan(e){if(this._pointers.length===1)this._panStart.set(e.pageX,e.pageY);else{let t=this._getSecondPointerPosition(e),i=.5*(e.pageX+t.x),r=.5*(e.pageY+t.y);this._panStart.set(i,r)}}_handleTouchStartDolly(e){let t=this._getSecondPointerPosition(e),i=e.pageX-t.x,r=e.pageY-t.y,s=Math.sqrt(i*i+r*r);this._dollyStart.set(0,s)}_handleTouchStartDollyPan(e){this.enableZoom&&this._handleTouchStartDolly(e),this.enablePan&&this._handleTouchStartPan(e)}_handleTouchStartDollyRotate(e){this.enableZoom&&this._handleTouchStartDolly(e),this.enableRotate&&this._handleTouchStartRotate(e)}_handleTouchMoveRotate(e){if(this._pointers.length==1)this._rotateEnd.set(e.pageX,e.pageY);else{let i=this._getSecondPointerPosition(e),r=.5*(e.pageX+i.x),s=.5*(e.pageY+i.y);this._rotateEnd.set(r,s)}this._rotateDelta.subVectors(this._rotateEnd,this._rotateStart).multiplyScalar(this.rotateSpeed);let t=this.domElement;this._rotateLeft(Hn*this._rotateDelta.x/t.clientHeight),this._rotateUp(Hn*this._rotateDelta.y/t.clientHeight),this._rotateStart.copy(this._rotateEnd)}_handleTouchMovePan(e){if(this._pointers.length===1)this._panEnd.set(e.pageX,e.pageY);else{let t=this._getSecondPointerPosition(e),i=.5*(e.pageX+t.x),r=.5*(e.pageY+t.y);this._panEnd.set(i,r)}this._panDelta.subVectors(this._panEnd,this._panStart).multiplyScalar(this.panSpeed),this._pan(this._panDelta.x,this._panDelta.y),this._panStart.copy(this._panEnd)}_handleTouchMoveDolly(e){let t=this._getSecondPointerPosition(e),i=e.pageX-t.x,r=e.pageY-t.y,s=Math.sqrt(i*i+r*r);this._dollyEnd.set(0,s),this._dollyDelta.set(0,Math.pow(this._dollyEnd.y/this._dollyStart.y,this.zoomSpeed)),this._dollyOut(this._dollyDelta.y),this._dollyStart.copy(this._dollyEnd);let o=(e.pageX+t.x)*.5,a=(e.pageY+t.y)*.5;this._updateZoomParameters(o,a)}_handleTouchMoveDollyPan(e){this.enableZoom&&this._handleTouchMoveDolly(e),this.enablePan&&this._handleTouchMovePan(e)}_handleTouchMoveDollyRotate(e){this.enableZoom&&this._handleTouchMoveDolly(e),this.enableRotate&&this._handleTouchMoveRotate(e)}_addPointer(e){this._pointers.push(e.pointerId)}_removePointer(e){delete this._pointerPositions[e.pointerId];for(let t=0;t<this._pointers.length;t++)if(this._pointers[t]==e.pointerId){this._pointers.splice(t,1);return}}_isTrackingPointer(e){for(let t=0;t<this._pointers.length;t++)if(this._pointers[t]==e.pointerId)return!0;return!1}_trackPointer(e){let t=this._pointerPositions[e.pointerId];t===void 0&&(t=new Xe,this._pointerPositions[e.pointerId]=t),t.set(e.pageX,e.pageY)}_getSecondPointerPosition(e){let t=e.pointerId===this._pointers[0]?this._pointers[1]:this._pointers[0];return this._pointerPositions[t]}_customWheelEvent(e){let t=e.deltaMode,i={clientX:e.clientX,clientY:e.clientY,deltaY:e.deltaY};switch(t){case 1:i.deltaY*=16;break;case 2:i.deltaY*=100;break}return e.ctrlKey&&!this._controlActive&&(i.deltaY*=10),i}};function u1(n){this.enabled!==!1&&(this._pointers.length===0&&(this.domElement.setPointerCapture(n.pointerId),this.domElement.ownerDocument.addEventListener("pointermove",this._onPointerMove),this.domElement.ownerDocument.addEventListener("pointerup",this._onPointerUp)),!this._isTrackingPointer(n)&&(this._addPointer(n),n.pointerType==="touch"?this._onTouchStart(n):this._onMouseDown(n),this._cursorStyle==="grab"&&(this.domElement.style.cursor="grabbing")))}function h1(n){this.enabled!==!1&&(n.pointerType==="touch"?this._onTouchMove(n):this._onMouseMove(n))}function d1(n){switch(this._removePointer(n),this._pointers.length){case 0:this.domElement.releasePointerCapture(n.pointerId),this.domElement.ownerDocument.removeEventListener("pointermove",this._onPointerMove),this.domElement.ownerDocument.removeEventListener("pointerup",this._onPointerUp),this.dispatchEvent(Xd),this.state=Tt.NONE,this._cursorStyle==="grab"&&(this.domElement.style.cursor="grab");break;case 1:let e=this._pointers[0],t=this._pointerPositions[e];this._onTouchStart({pointerId:e,pageX:t.x,pageY:t.y});break}}function f1(n){let e;switch(n.button){case 0:e=this.mouseButtons.LEFT;break;case 1:e=this.mouseButtons.MIDDLE;break;case 2:e=this.mouseButtons.RIGHT;break;default:e=-1}switch(e){case Qi.DOLLY:if(this.enableZoom===!1)return;this._handleMouseDownDolly(n),this.state=Tt.DOLLY;break;case Qi.ROTATE:if(n.ctrlKey||n.metaKey||n.shiftKey){if(this.enablePan===!1)return;this._handleMouseDownPan(n),this.state=Tt.PAN}else{if(this.enableRotate===!1)return;this._handleMouseDownRotate(n),this.state=Tt.ROTATE}break;case Qi.PAN:if(n.ctrlKey||n.metaKey||n.shiftKey){if(this.enableRotate===!1)return;this._handleMouseDownRotate(n),this.state=Tt.ROTATE}else{if(this.enablePan===!1)return;this._handleMouseDownPan(n),this.state=Tt.PAN}break;default:this.state=Tt.NONE}this.state!==Tt.NONE&&this.dispatchEvent(yu)}function p1(n){switch(this.state){case Tt.ROTATE:if(this.enableRotate===!1)return;this._handleMouseMoveRotate(n);break;case Tt.DOLLY:if(this.enableZoom===!1)return;this._handleMouseMoveDolly(n);break;case Tt.PAN:if(this.enablePan===!1)return;this._handleMouseMovePan(n);break}}function m1(n){this.enabled===!1||this.enableZoom===!1||this.state!==Tt.NONE||(n.preventDefault(),this.dispatchEvent(yu),this._handleMouseWheel(this._customWheelEvent(n)),this.dispatchEvent(Xd))}function g1(n){this.enabled!==!1&&this._handleKeyDown(n)}function x1(n){switch(this._trackPointer(n),this._pointers.length){case 1:switch(this.touches.ONE){case er.ROTATE:if(this.enableRotate===!1)return;this._handleTouchStartRotate(n),this.state=Tt.TOUCH_ROTATE;break;case er.PAN:if(this.enablePan===!1)return;this._handleTouchStartPan(n),this.state=Tt.TOUCH_PAN;break;default:this.state=Tt.NONE}break;case 2:switch(this.touches.TWO){case er.DOLLY_PAN:if(this.enableZoom===!1&&this.enablePan===!1)return;this._handleTouchStartDollyPan(n),this.state=Tt.TOUCH_DOLLY_PAN;break;case er.DOLLY_ROTATE:if(this.enableZoom===!1&&this.enableRotate===!1)return;this._handleTouchStartDollyRotate(n),this.state=Tt.TOUCH_DOLLY_ROTATE;break;default:this.state=Tt.NONE}break;default:this.state=Tt.NONE}this.state!==Tt.NONE&&this.dispatchEvent(yu)}function v1(n){switch(this._trackPointer(n),this.state){case Tt.TOUCH_ROTATE:if(this.enableRotate===!1)return;this._handleTouchMoveRotate(n),this.update();break;case Tt.TOUCH_PAN:if(this.enablePan===!1)return;this._handleTouchMovePan(n),this.update();break;case Tt.TOUCH_DOLLY_PAN:if(this.enableZoom===!1&&this.enablePan===!1)return;this._handleTouchMoveDollyPan(n),this.update();break;case Tt.TOUCH_DOLLY_ROTATE:if(this.enableZoom===!1&&this.enableRotate===!1)return;this._handleTouchMoveDollyRotate(n),this.update();break;default:this.state=Tt.NONE}}function y1(n){this.enabled!==!1&&n.preventDefault()}function _1(n){n.key==="Control"&&(this._controlActive=!0,this.domElement.getRootNode().addEventListener("keyup",this._interceptControlUp,{passive:!0,capture:!0}))}function b1(n){n.key==="Control"&&(this._controlActive=!1,this.domElement.getRootNode().removeEventListener("keyup",this._interceptControlUp,{passive:!0,capture:!0}))}var xo=new G;function Qn(n,e,t,i,r,s){let o=2*Math.PI*r/4,a=Math.max(s-2*r,0),l=Math.PI/4;xo.copy(e),xo[i]=0,xo.normalize();let c=.5*o/(o+a),h=1-xo.angleTo(n)/l;return Math.sign(xo[t])===1?h*c:a/(o+a)+c+c*(1-h)}var Ni=class n extends bt{constructor(e=1,t=1,i=1,r=2,s=.1){let o=r*2+1;if(s=Math.min(e/2,t/2,i/2,s),super(1,1,1,o,o,o),this.type="RoundedBoxGeometry",this.parameters={width:e,height:t,depth:i,segments:r,radius:s},o===1)return;let a=this.toNonIndexed();this.index=null,this.attributes.position=a.attributes.position,this.attributes.normal=a.attributes.normal,this.attributes.uv=a.attributes.uv;let l=new G,c=new G,h=new G(e,t,i).divideScalar(2).subScalar(s),d=this.attributes.position.array,u=this.attributes.normal.array,p=this.attributes.uv.array,x=d.length/6,v=new G,m=.5/o;for(let f=0,b=0;f<d.length;f+=3,b+=2)switch(l.fromArray(d,f),c.copy(l),c.x-=Math.sign(c.x)*m,c.y-=Math.sign(c.y)*m,c.z-=Math.sign(c.z)*m,c.normalize(),d[f+0]=h.x*Math.sign(l.x)+c.x*s,d[f+1]=h.y*Math.sign(l.y)+c.y*s,d[f+2]=h.z*Math.sign(l.z)+c.z*s,u[f+0]=c.x,u[f+1]=c.y,u[f+2]=c.z,Math.floor(f/x)){case 0:v.set(1,0,0),p[b+0]=Qn(v,c,"z","y",s,i),p[b+1]=1-Qn(v,c,"y","z",s,t);break;case 1:v.set(-1,0,0),p[b+0]=1-Qn(v,c,"z","y",s,i),p[b+1]=1-Qn(v,c,"y","z",s,t);break;case 2:v.set(0,1,0),p[b+0]=1-Qn(v,c,"x","z",s,e),p[b+1]=Qn(v,c,"z","x",s,i);break;case 3:v.set(0,-1,0),p[b+0]=1-Qn(v,c,"x","z",s,e),p[b+1]=1-Qn(v,c,"z","x",s,i);break;case 4:v.set(0,0,1),p[b+0]=1-Qn(v,c,"x","y",s,e),p[b+1]=1-Qn(v,c,"y","x",s,t);break;case 5:v.set(0,0,-1),p[b+0]=Qn(v,c,"x","y",s,e),p[b+1]=1-Qn(v,c,"y","x",s,t);break}}static fromJSON(e){return new n(e.width,e.height,e.depth,e.segments,e.radius)}};function qd(n,e=1e-4){e=Math.max(e,Number.EPSILON);let t={},i=n.getIndex(),r=n.getAttribute("position"),s=i?i.count:r.count,o=0,a=Object.keys(n.attributes),l={},c={},h=[],d=["getX","getY","getZ","getW"],u=["setX","setY","setZ","setW"];for(let b=0,P=a.length;b<P;b++){let w=a[b],T=n.attributes[w];l[w]=new T.constructor(new T.array.constructor(T.count*T.itemSize),T.itemSize,T.normalized);let E=n.morphAttributes[w];E&&(c[w]||(c[w]=[]),E.forEach((D,g)=>{let S=new D.array.constructor(D.count*D.itemSize);c[w][g]=new D.constructor(S,D.itemSize,D.normalized)}))}let p=e*.5,x=Math.log10(1/e),v=Math.pow(10,x),m=p*v;for(let b=0;b<s;b++){let P=i?i.getX(b):b,w="";for(let T=0,E=a.length;T<E;T++){let D=a[T],g=n.getAttribute(D),S=g.itemSize;for(let _=0;_<S;_++)w+=`${Math.trunc(g[d[_]](P)*v+m)},`}if(w in t)h.push(t[w]);else{for(let T=0,E=a.length;T<E;T++){let D=a[T],g=n.getAttribute(D),S=n.morphAttributes[D],_=g.itemSize,A=l[D],I=c[D];for(let k=0;k<_;k++){let N=d[k],U=u[k];if(A[U](o,g[N](P)),S)for(let z=0,Y=S.length;z<Y;z++)I[z][U](o,S[z][N](P))}}t[w]=o,h.push(o),o++}}let f=n.clone();for(let b in n.attributes){let P=l[b];if(f.setAttribute(b,new P.constructor(P.array.slice(0,o*P.itemSize),P.itemSize,P.normalized)),b in c)for(let w=0;w<c[b].length;w++){let T=c[b][w];f.morphAttributes[b][w]=new T.constructor(T.array.slice(0,o*T.itemSize),T.itemSize,T.normalized)}}return f.setIndex(h),f}var M1=new ke(2827296),or=null;function S1(){if(or)return or;let n=[90,170,255],e=new Uint8Array(n.length*4);return n.forEach((t,i)=>e.set([t,t,t,255],i*4)),or=new vr(e,n.length,1,Bn),or.minFilter=en,or.magFilter=en,or.generateMipmaps=!1,or.needsUpdate=!0,or}function it(n={}){let{roughness:e,metalness:t,...i}=n;return new Qs({gradientMap:S1(),...i})}var w1=`
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
}`,E1=`
uniform vec3 color;
#include <common>
#include <fog_pars_fragment>
void main() {
  gl_FragColor = vec4(color, 1.0);
  #include <fog_fragment>
}`,Pl=null;function T1(){return Pl||(Pl=new En({uniforms:bl.merge([Te.fog,{thickness:{value:.024},color:{value:M1}}]),vertexShader:w1,fragmentShader:E1,side:mn,fog:!0}),Pl)}var $d=new WeakMap;function A1(n){let e=$d.get(n);if(e)return e;let t=new Yt;t.setAttribute("position",n.getAttribute("position")),n.index&&t.setIndex(n.index);let i=qd(t,.001);return i.computeVertexNormals(),$d.set(n,i),i}function ei(n,e=.12){let t=[];n.traverse(i=>{let r=i;if(!r.isMesh||r.userData.outline===!1||r.userData.isOutline||r.children.some(a=>a.userData.isOutline))return;let s=Array.isArray(r.material)?r.material[0]:r.material;if(!s||s.transparent||s.isMeshBasicMaterial)return;let o=r.geometry;o.boundingSphere||o.computeBoundingSphere(),!((o.boundingSphere?.radius??0)<e)&&t.push(r)});for(let i of t){let r=new $e(A1(i.geometry),T1());r.userData.isOutline=!0,r.castShadow=!1,r.receiveShadow=!1,r.raycast=()=>{},i.add(r)}}var R1=`
varying vec3 vDir;
void main() {
  vDir = normalize(position);
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  gl_Position.z = gl_Position.w; // always at the far plane
}`,C1=`
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
}`;function Yd(n,e,t){let i=new En({uniforms:{top:{value:n},horizon:{value:e},ground:{value:t}},vertexShader:R1,fragmentShader:C1,side:mn,depthWrite:!1}),r=new $e(new Lt(400,32,16),i);return r.renderOrder=-1,r.frustumCulled=!1,r}function Zd(n,e,t){let i=new Oe,r=it({color:16777215,emissive:14542576,emissiveIntensity:.35}),s=new Lt(1,16,12);for(let o=0;o<n;o++){let a=new Oe,l=4+Math.floor(t()*3);for(let d=0;d<l;d++){let u=1.6+t()*1.6,p=new $e(s,r);p.scale.set(u*1.3,u*.8,u),p.position.set((d-l/2)*1.9+t(),t()*.8,(t()-.5)*2),a.add(p)}let c=o/n*Math.PI*2+t()*.4,h=e*(.7+t()*.5);a.position.set(Math.cos(c)*h,48+t()*14,Math.sin(c)*h),a.rotation.y=t()*Math.PI,i.add(a)}return ei(i,0),{group:i,animate:o=>i.rotation.y=o*.006}}function Kd(n,e,t,i){let r=[];for(let x=0;x<3;x++){let v=x/3*Math.PI+.3,m=Math.cos(v)*.07,f=Math.sin(v)*.07,b=(x-1)*.06;r.push(-m,0,-f,m,0,f,b,.32+x*.05,0)}let s=new Yt;s.setAttribute("position",new _t(r,3)),s.computeVertexNormals();let o=it({color:16777215,side:Fn}),a={uTime:{value:0}};o.onBeforeCompile=x=>{x.uniforms.uTime=a.uTime,x.vertexShader=x.vertexShader.replace("#include <common>",`#include <common>
uniform float uTime;`).replace("#include <begin_vertex>",`#include <begin_vertex>
        vec4 wp = instanceMatrix * vec4(0.0, 0.0, 0.0, 1.0);
        float sway = sin(uTime * 1.7 + wp.x * 0.35 + wp.z * 0.25) * 0.5 + sin(uTime * 3.1 + wp.x) * 0.15;
        transformed.x += sway * position.y * 0.45;
        transformed.z += sway * position.y * 0.2;`),x.vertexShader=x.vertexShader.replace("#include <color_vertex>",`#include <color_vertex>
vTipShade = position.y;`),x.vertexShader=x.vertexShader.replace("#include <common>",`#include <common>
varying float vTipShade;`),x.fragmentShader=x.fragmentShader.replace("#include <common>",`#include <common>
varying float vTipShade;`).replace("#include <color_fragment>",`#include <color_fragment>
diffuseColor.rgb *= 0.75 + vTipShade * 1.1;`)};let l=new qs(s,o,n),c=new wt,h=new un,d=new G(0,1,0),u=new ke,p=0;for(let x=0;x<n*4&&p<n;x++){let v=(i()*2-1)*e,m=(i()*2-1)*e;if(Math.hypot(v,m)>e||!t(v,m))continue;let f=.7+i()*.9;h.setFromAxisAngle(d,i()*Math.PI*2),c.compose(new G(v,0,m),h,new G(f,f,f)),l.setMatrixAt(p,c),l.setColorAt(p,u.setHSL(.24+i()*.07,.5+i()*.2,.42+i()*.14)),p++}return l.count=p,l.receiveShadow=!0,l.instanceMatrix.needsUpdate=!0,l.instanceColor&&(l.instanceColor.needsUpdate=!0),l.userData.outline=!1,{mesh:l,animate:x=>a.uTime.value=x}}var vo=.46,P1=.5,I1=.78,k1=.82,yo=.24,Jd=1.3,L1=1.25*Jd,D1=.35;function Il(n,e,t,i=n){let r=new Oe;r.name=e?"robot-orchestrator":"robot",r.scale.setScalar(e?L1:Jd);let s=[],o=[],a=(fe,Ie,We=.1)=>{let He=it({color:fe,roughness:Ie,metalness:We});return s.push(He),o.push(He),He},l=a(new ke().setHSL(i/360,.6,.64,Pt),.65),c=a(t.paper,.6),h=a(new ke().setHSL(n/360,.5,.82,Pt),.5),d=a(t.rule,.45),u=a(t.ink,.8,.05),p=a(t.signal,.5),x=it({color:t.ink,roughness:.12,metalness:.4}),v=it({color:t.ok,emissive:t.ok,emissiveIntensity:1.2,roughness:.3}),m=it({color:t.signal,emissive:t.signal,emissiveIntensity:1.2,roughness:.4});s.push(x,v,m);let f=(fe,Ie,We,He=0,oe=0,le=0)=>{let be=new $e(Ie,We);return be.position.set(He,oe,le),be.castShadow=!0,be.receiveShadow=!0,fe.add(be),be},b=new Oe;r.add(b);let P=new Ni(.16,.22,.86,3,.07);f(b,P,u,-.36,.11,0),f(b,P,u,.36,.11,0);let w=new Wt(.08,.08,.03,16);w.rotateZ(Math.PI/2);let T=[];for(let fe of[-1,1])for(let Ie of[-.28,0,.28])T.push(f(b,w,d,fe*.445,.11,Ie));let E=new Oe;r.add(E),f(E,new Ni(.62,P1,.6,4,.12),l,0,vo,0),f(E,new Ni(.36,.26,.03,2,.01),h,0,vo,.315),f(E,new bt(.28,.012,.012),d,0,vo,.33);let D=new Lt(.018,8,6);for(let fe of[-.15,.15])for(let Ie of[vo-.1,vo+.1])f(E,D,d,fe,Ie,.33);f(E,new Wt(.07,.08,.14,12),u,0,I1,0);let g=new Wt(.045,.04,.3,12);g.translate(0,-.15,0);let S=new Ni(.1,.05,.08,2,.02),_=new bt(.025,.08,.03),A=fe=>{let Ie=new Oe;Ie.position.set(fe*.34,.6,0),E.add(Ie),f(Ie,g,l);let We=new Oe;return We.position.set(0,-.3,0),Ie.add(We),f(We,S,u),f(We,_,u,-.035,-.05,0),f(We,_,u,.035,-.05,0),Ie},I=A(-1),k=A(1),N=new Oe;N.position.set(0,k1,0),E.add(N),f(N,new Ni(.62,yo,.42,3,.09),c,0,yo/2,0);let U=new Wt(.13,.13,.18,24);U.rotateX(Math.PI/2);let z=new In(.1,24),Y=new In(.07,24),ee=[];for(let fe of[-.16,.16])f(N,U,c,fe,.12,.2),f(N,z,x,fe,.12,.292),ee.push(f(N,Y,v,fe,.12,.296));let J;e&&(f(N,new Wt(.01,.01,.16,6),u,.22,yo+.08,-.05),J=f(N,new Lt(.03,10,8),m,.22,yo+.17,-.05),f(E,new Ni(.36,.03,.02,2,.008),p,0,.62,.31));let ie="idle",ce=1.2,Se=fe=>{ie=fe;let Ie=fe==="offline"?t.inkMuted:fe==="working"?t.info:t.ok;v.color.set(Ie),v.emissive.set(Ie),ce=fe==="offline"?.2:fe==="working"?2:1.3};return Se("idle"),ei(r,.03),{group:r,labelAnchor(fe){return N.updateWorldMatrix(!0,!1),N.localToWorld(fe.set(0,yo+D1,0))},setState(fe){fe!==ie&&Se(fe)},update(fe,Ie){let We=Math.min(1,fe*8),He=ie,oe=0,le=0;He==="idle"?(oe=.03*Math.sin(Ie*.5),le=.12*Math.sin(Ie*.9)):He==="working"?(oe=.14+.08*Math.sin(Ie*5),le=.04*Math.sin(Ie*1.3)):He==="walking"?(oe=.04,le=.06*Math.sin(Ie*3)):(oe=.5,le=.12),N.rotation.x+=(oe-N.rotation.x)*We,N.rotation.z+=(le-N.rotation.z)*We;let be=0,Ve=0;if(He==="idle"?be=.012*Math.sin(Ie*1.6):He==="working"?be=.004*Math.sin(Ie*2.5):He==="walking"?(be=.01*Math.abs(Math.sin(Ie*6)),Ve=.05*Math.sin(Ie*6)):be=-.02,E.position.y=be,E.rotation.z=Ve,b.position.y=He==="walking"?.012*Math.abs(Math.sin(Ie*10)):0,He==="walking")for(let Ge of T)Ge.rotation.x+=fe*8;if(He==="working"){let Ge=Math.sin(Ie*11);I.rotation.x=-.9+.2*Ge,k.rotation.x=-.9-.2*Ge}else if(He==="walking"){let Ge=.25*Math.sin(Ie*6);I.rotation.x=Ge,k.rotation.x=-Ge}else if(He==="idle"){let Ge=.05*Math.sin(Ie*1.1);I.rotation.x=Ge,k.rotation.x=-Ge}else I.rotation.x=.2,k.rotation.x=.2;let Me=1;He==="idle"&&Ie%3.6<.14?Me=.1:He==="offline"&&(Me=.45);for(let Ge of ee)Ge.scale.y=Me;v.emissiveIntensity=He==="working"?ce+.3*Math.sin(Ie*5):ce,J&&(m.emissiveIntensity=1.1+.5*Math.sin(Ie*3))},pose(fe){I.rotation.set(0,0,0),k.rotation.set(0,0,0),N.rotation.set(0,0,0),fe==="wave"?(k.rotation.z=2.5,I.rotation.z=-.25,N.rotation.z=-.15):fe==="proud"?(I.rotation.z=-.55,k.rotation.z=.55,N.rotation.x=-.12):fe==="think"?(k.rotation.x=-1.9,k.rotation.z=-.35,N.rotation.z=.2,N.rotation.x=.08):(I.rotation.z=-2.7,k.rotation.z=2.7,N.rotation.x=-.15)},setHighlighted(fe){for(let Ie of o)Ie.emissive.set(fe?t.signal:0),Ie.emissiveIntensity=fe?.35:0},dispose(){r.traverse(fe=>{fe instanceof $e&&fe.geometry.dispose()});for(let fe of s)fe.dispose()}}}var jd=new Map,Qd=new Map;function Ui(n){let e=n>>>0;return()=>{e=e+1831565813>>>0;let t=e;return t=Math.imul(t^t>>>15,t|1),t^=t+Math.imul(t^t>>>7,t|61),((t^t>>>14)>>>0)/4294967296}}function N1(n){let e=2166136261;for(let t=0;t<n.length;t++)e=Math.imul(e^n.charCodeAt(t),16777619);return e>>>0}function ti(n,e,t,i,r){let s=jd.get(n);if(s)return s;let o=document.createElement("canvas");o.width=e,o.height=t;let a=o.getContext("2d");a&&r(a,e,t,Ui(N1(n)));let l=new Ii(o);return l.wrapS=gr,l.wrapT=gr,l.repeat.set(i,i),l.colorSpace=Pt,l.anisotropy=4,jd.set(n,l),l}var _o=n=>n.getStyle();function _u(n,e,t,i,r,s){n.beginPath(),n.moveTo(e+s,t),n.arcTo(e+i,t,e+i,t+r,s),n.arcTo(e+i,t+r,e,t+r,s),n.arcTo(e,t+r,e,t,s),n.arcTo(e,t,e+i,t,s),n.closePath()}function bu(){return ti("grass",512,512,24,(n,e,t,i)=>{n.fillStyle="#86c45e",n.fillRect(0,0,e,t);for(let r=0;r<14;r++)n.fillStyle=i()<.5?"#8fcb66":"#7fbc58",n.beginPath(),n.ellipse(i()*e,i()*t,30+i()*60,20+i()*40,i()*Math.PI,0,Math.PI*2),n.fill();n.lineCap="round";for(let r=0;r<90;r++){let s=i()*e,o=i()*t,a=4+i()*5;n.strokeStyle=i()<.7?"rgba(70,128,52,0.55)":"rgba(196,232,150,0.7)",n.lineWidth=1.6,n.beginPath(),n.moveTo(s-a*.6,o-a),n.lineTo(s,o),n.lineTo(s+a*.1,o-a*1.3),n.moveTo(s,o),n.lineTo(s+a*.7,o-a*.9),n.stroke()}for(let r=0;r<12;r++)n.fillStyle=["#fff6d8","#ffd25e","#ff9db5"][Math.floor(i()*3)],n.beginPath(),n.arc(i()*e,i()*t,2+i()*1.5,0,Math.PI*2),n.fill()})}function bo(n,e=4){return ti(`paving:${_o(n)}:${e}`,256,256,e,(t,i,r,s)=>{let a=r/5,l=i/4;t.fillStyle=_o(n.clone().multiplyScalar(.55)),t.fillRect(0,0,i,r);for(let c=0;c<5;c++){let h=c%2*(l/2);for(let d=-1;d<=4;d++){let u=d*l+h+3+s()*2,p=c*a+3+s()*2,x=l-7,v=a-7;_u(t,u,p,x,v,9),t.fillStyle=_o(n.clone().multiplyScalar(.92+s()*.14)),t.fill(),t.lineWidth=2,t.strokeStyle="rgba(43,36,32,0.55)",t.stroke(),_u(t,u+5,p+4,x*.45,5,2.5),t.fillStyle="rgba(255,255,255,0.45)",t.fill()}}})}function Mo(){return ti("wood",256,256,1,(n,e,t,i)=>{let s=t/4;for(let o=0;o<4;o++){n.fillStyle=`hsl(${28+i()*6}, ${55+i()*10}%, ${50+i()*8}%)`,n.fillRect(0,o*s,e,s),n.fillStyle="rgba(255,255,255,0.18)",n.fillRect(0,o*s+4,e,5);for(let a=0;a<2;a++)n.strokeStyle="rgba(90,50,20,0.45)",n.lineWidth=2,n.beginPath(),n.ellipse(i()*e,o*s+s/2,7+i()*5,3,0,0,Math.PI*2),n.stroke();n.fillStyle="rgba(60,32,14,0.8)",n.fillRect(0,o*s,e,3)}})}function Mu(){return ti("metal",64,64,1,(n,e,t)=>{n.fillStyle="#8f9bab",n.fillRect(0,0,e,t),n.fillStyle="rgba(255,255,255,0.35)",n.fillRect(0,t*.2,e,t*.12)})}function ef(n){return ti(`polish:${_o(n)}`,256,256,2,(e,t,i,r)=>{e.fillStyle=_o(n),e.fillRect(0,0,t,i);for(let s=0;s<14;s++)e.fillStyle="rgba(255,255,255,0.35)",_u(e,r()*t,r()*i,18+r()*30,5,2.5),e.fill();for(let s=0;s<10;s++)e.fillStyle="rgba(120,100,80,0.12)",e.beginPath(),e.arc(r()*t,r()*i,8+r()*14,0,Math.PI*2),e.fill()})}function tf(){return ti("water",256,256,2,(n,e,t,i)=>{n.fillStyle="#3cc0d6",n.fillRect(0,0,e,t),n.fillStyle="#5fd3e3";for(let r=0;r<10;r++)n.beginPath(),n.ellipse(i()*e,i()*t,30+i()*30,12+i()*10,0,0,Math.PI*2),n.fill();n.lineCap="round";for(let r=0;r<26;r++){let s=i()*e,o=i()*t;n.strokeStyle="rgba(255,255,255,0.85)",n.lineWidth=3,n.beginPath(),n.moveTo(s,o),n.quadraticCurveTo(s+8,o-6,s+16,o),n.quadraticCurveTo(s+24,o+6,s+32,o),n.stroke()}})}function nf(n){let e=`side:${Math.round(n)}`,t=Qd.get(e);if(t)return t;let i=it({color:new ke().setHSL(n/360,.35,.66,Pt)});return Qd.set(e,i),i}var U1=new ke(16766874),F1=new ke(13208422);function di(n,e,t=.1){return it({color:n,roughness:e,metalness:t})}function At(n,e,t,i,r,s){let o=new $e(e,t);return o.position.set(i,r,s),n.add(o),o}function bs(n){n.traverse(e=>{e instanceof $e&&(e.castShadow=!0,e.receiveShadow=!0)})}function Ar(n){n.updateWorldMatrix(!0,!1);let e=new Nn().setFromObject(n);return{x0:e.min.x,x1:e.max.x,z0:e.min.z,z1:e.max.z}}function Su(n,e){return n[Math.floor(e()*n.length)]}function rf(n,e){return Math.abs(Math.atan2(Math.sin(n-e),Math.cos(n-e)))}function O1(n,e,t,i){let r=Math.max(t*t+i*i,1e-6),s=Math.max(0,Math.min(1,(n*t+e*i)/r));return Math.hypot(n-t*s,e-i*s)}function sf(n){let e=new Oe;return At(e,new bt(1.3,.08,.42),n,0,.42,0),At(e,new bt(.08,.4,.36),n,-.55,.2,0),At(e,new bt(.08,.4,.36),n,.55,.2,0),At(e,new bt(1.3,.4,.07),n,0,.66,-.2),e}function of(n){let e=new Oe,t=n.rule.clone().lerp(n.sheet,.4),i=At(e,new Wt(4.3,4.45,.06,64),it({map:bo(t,4),roughness:.9}),0,.03,0);i.receiveShadow=!0;let r=At(e,new Js(3.7,3.95,72),di(t.clone().multiplyScalar(.72),.9,0),0,.066,0);r.rotation.x=-Math.PI/2;let s=At(e,new Un(3.3,.025,6,72),di(n.rule,.8,0),0,.065,0);s.rotation.x=Math.PI/2;let o=it({map:Mo(),color:n.rule.clone().lerp(n.ink3,.25),roughness:.8});for(let D=0;D<4;D++){let g=D/4*Math.PI*2,S=Math.cos(g)*3.6,_=Math.sin(g)*3.6,A=sf(o);A.position.set(S,0,_),A.rotation.y=Math.atan2(-S,-_),e.add(A)}let a=new ke(10463487),l=new Oe,c=new Oe,h=it({color:n.signal,emissive:n.signal,emissiveIntensity:.45}),d=it({color:n.sheet,emissive:16769712,emissiveIntensity:.25}),u=new Lt(.13,14,10),p=Ui(54);for(let D=0;D<24;D++){let g=new G(p()-.5,p()-.5,p()-.5).normalize().multiplyScalar(Math.cbrt(p())*.24);At(c,u,D%2?h:d,g.x,g.y,g.z)}l.add(c);let x=new Ot({color:a,transparent:!0,opacity:.35,depthWrite:!1,blending:Li,toneMapped:!1}),v=new $e(new Lt(.62,24,16),x);l.add(v);let m=new Ot({color:a,transparent:!0,opacity:.1,depthWrite:!1,blending:Li,toneMapped:!1}),f=new $e(new Lt(1.9,24,16),m);l.add(f);let b=new ki(a,2.2,9,1.6);l.add(b);let P=it({color:13225215,emissive:a,emissiveIntensity:.6}),w=it({color:9433343,emissive:6281471,emissiveIntensity:1.1}),T=new Lt(.065,12,8),E=[];return[2,8,18,18,8].forEach((D,g)=>{let S=.62+g*.27,_=new Oe;_.rotation.set(p()*Math.PI,p()*Math.PI,p()*Math.PI);let A=new $e(new Un(S,.012,6,96),P);A.rotation.x=Math.PI/2,A.userData.outline=!1,_.add(A);let I=[];for(let k=0;k<D;k++)I.push(At(_,T,w,0,0,0));l.add(_),E.push({group:_,electrons:I,r:S,speed:1.9/(1+g*.45)*(g%2?-1:1)})}),ei(l,.05),l.scale.setScalar(1.5),l.position.set(0,4.2,0),e.add(l),e.userData={animate:D=>{l.position.y=4.2+Math.sin(D*.9)*.1;let g=Math.pow(Math.max(0,Math.sin(D*2.4)),6);c.scale.setScalar(1+.12*g),c.rotation.y=D*.6,v.scale.setScalar(1+.35*g+.05*Math.sin(D*7)),x.opacity=.3+.35*g,m.opacity=.07+.08*g,f.scale.setScalar(1+.1*g),b.intensity=1.6+2.4*g;for(let S of E){S.group.rotation.y+=.002;let _=S.electrons.length;S.electrons.forEach((A,I)=>{let k=D*S.speed+I/_*Math.PI*2;A.position.set(Math.cos(k)*S.r,0,Math.sin(k)*S.r),A.scale.setScalar(1+.25*g)})}}},bs(e),l.traverse(D=>D.castShadow=!1),ei(e),e}function af(n,e){let t=new Oe,i=[],r=4,s=[];for(let x=0;x<8;x++){let v=Math.PI/8+x*Math.PI/4;e.some(m=>rf(v,m)<.3)||s.every(m=>rf(v,m)>1)&&s.push(v)}let o=tf(),a=it({map:ef(n.sheet),roughness:.4}),l=it({map:o,roughness:.12,transparent:!0,opacity:.9}),c=it({map:Mo(),roughness:.8}),h=it({map:Mu(),roughness:.4,metalness:.5}),d=ti(`awning:${n.signal.getStyle()}:${n.sheet.getStyle()}`,256,64,1,(x,v,m)=>{for(let b=0;b<8;b++)x.fillStyle=b%2===1?n.signal.getStyle():n.sheet.getStyle(),x.fillRect(b*v/8,0,v/8,m)}),u=it({map:d,roughness:.9,side:Fn});s.forEach((x,v)=>{let m=Math.cos(x)*r,f=Math.sin(x)*r,b=new Oe;if(b.position.set(m,0,f),v===0){let P=At(b,new Wt(.62,.7,.26,32),a,0,.13,0),w=At(b,new In(.55,32),l,0,.27,0);w.rotation.x=-Math.PI/2,At(b,new Wt(.05,.07,.6,8),a,0,.56,0),At(b,new Lt(.11,12,8),l,0,.92,0),i.push(Ar(P))}else{b.rotation.y=Math.atan2(-m,-f);let P=At(b,new bt(1.1,.72,.5),c,0,.36,0);At(b,new bt(.26,.18,.26),di(n.ok,.7,0),.25,.81,0),At(b,new bt(.24,.16,.24),di(n.signal,.7,0),-.3,.8,.05);for(let T of[-.5,.5])At(b,new Wt(.03,.03,2,6),h,T,1,.42);let w=At(b,new bt(1.4,.04,.95),u,0,2,.1);w.rotation.x=.2,i.push(Ar(P))}bs(b),t.add(b)});let p=x=>{o.offset.y=x*.04%1};return ei(t),{group:t,boxes:i,animate:p}}function lf(n,e){let t=new Oe,i=it({map:bo(e.rule.clone().lerp(e.sheet,.35),2),roughness:.95});for(let r of n){let s=Math.hypot(r.x,r.z);if(s<.5)continue;let o=new bt(s,.03,.9),a=o.getAttribute("uv");for(let c=0;c<a.count;c++)a.setX(c,a.getX(c)*(s/.9));let l=new $e(o,i);l.position.set(r.x/2,.015,r.z/2),l.rotation.y=-Math.atan2(r.z,r.x),l.receiveShadow=!0,t.add(l)}return t}function cf(n,e){let t=new Oe,i=[],r=Ui(1177),s=0;for(let S of n)s=Math.max(s,Math.hypot(S.cx,S.cy)+S.r);let o=Math.min(34,s+6),a=it({map:Mo(),roughness:.9}),l=[6262602,7840346,5077564,9416286].map(S=>di(new ke(S),.85,0)),c=[3104058,3960391].map(S=>di(new ke(S),.9,0)),h=[15907014,16245738].map(S=>di(new ke(S),.85,0)),d=[15909198,15305376,16249832,14243644,10190801].map(S=>di(new ke(S),.7,0)),u=di(e.rule.clone().lerp(e.ink3,.6),1,0),p=di(F1,.9,0),x=di(e.ink3.clone().lerp(e.sheet,.45),.95,0),v=it({map:Mo(),roughness:.8}),m=it({map:Mu(),roughness:.4,metalness:.5}),f=new Ot({color:U1,toneMapped:!1}),b=(S,_,A)=>{if(Math.hypot(S,_)<6.4+A)return!1;for(let I of n)if(Math.abs(S-I.cx)<I.r+3+A&&Math.abs(_-I.cy)<I.r+3+A||O1(S,_,I.cx,I.cy)<1.4+A)return!1;for(let I of i)if(S+A>I.x0-.3&&S-A<I.x1+.3&&_+A>I.z0-.3&&_-A<I.z1+.3)return!1;return!0},P=[],w=(S,_)=>{if(P.some(([k,N])=>Math.hypot(k-S,N-_)<3))return;P.push([S,_]);let A=new Oe;A.position.set(S,0,_);let I=At(A,new Wt(.035,.045,2.2,8),m,0,1.1,0);At(A,new Wt(.06,.08,.1,10),m,0,2.25,0),At(A,new Lt(.16,14,10),f,0,2.4,0),t.add(A),bs(A),i.push(Ar(I))},T=(S,_)=>{let A=new Oe;A.position.set(S,0,_);let I=.9+r()*.5,k=At(A,new Wt(.08,.12,I,8),a,0,I/2,0);if(r()<.35){let N=Su(c,r);for(let U=0;U<3;U++)At(A,new yr(.8-U*.2,.95,10),N,0,I+.25+U*.42,0)}else{let N=Su(r()<.2?h:l,r);At(A,new Lt(.72,14,10),N,0,I+.35,0),At(A,new Lt(.5,12,8),N,.28,I+.8,.12),At(A,new Lt(.42,12,8),N,-.22,I+.7,-.18)}t.add(A),bs(A),i.push(Ar(k))},E=(S,_)=>{let A=new Oe;A.position.set(S,0,_),A.rotation.y=r()*Math.PI;let I=At(A,new bt(1.5,.2,.9),p,0,.1,0);At(A,new bt(1.36,.06,.76),u,0,.19,0);for(let k=0;k<16;k++){let N=Su(d,r);At(A,new Lt(.06+r()*.03,8,6),N,(r()-.5)*1.1,.3+r()*.06,(r()-.5)*.6)}t.add(A),bs(A),i.push(Ar(I))},D=(S,_)=>{let A=.3+r()*.35,I=new $e(new Ks(A,0),x);I.position.set(S,A*.4,_),I.rotation.set(r()*3,r()*3,r()*3),I.castShadow=!0,I.receiveShadow=!0,t.add(I),i.push(Ar(I))},g=(S,_)=>{let A=sf(v);A.position.set(S,0,_),A.rotation.y=r()*Math.PI*2,t.add(A),bs(A),i.push(Ar(A))};n.forEach((S,_)=>{let A=Math.hypot(S.cx,S.cy),I=A-S.r-2.6;if(A<1||I<7)return;let k=S.cx/A,N=S.cy/A,U=_%2===0?1:-1;w(k*I-N*U*.95,N*I+k*U*.95)});for(let S=0;S<320;S++){let _=(r()*2-1)*o,A=(r()*2-1)*o,I=r();I<.42?b(_,A,.7)&&T(_,A):I<.62?b(_,A,.85)&&E(_,A):I<.84?b(_,A,.4)&&D(_,A):b(_,A,.7)&&g(_,A)}return ei(t,.1),{group:t,boxes:i}}var Ll=class{constructor(e,t,i,r,s){this.g=e;this.s=t;this.hue=i;this.k=r;this.rand=s}g;s;hue;k;rand;anim=[];item(e,t,i=0){let r=new Oe;return r.position.set(e,0,t),r.rotation.y=i,this.g.add(r),r}box(e,t,i,r,s,o=0,a=0,l=0){return kl(e,new bt(t,i,r),s,o,a+i/2,l)}cyl(e,t,i,r,s,o=0,a=0,l=0,c=16){return kl(e,new Wt(t,i,r,c),s,o,a+r/2,l)}ball(e,t,i,r=0,s=0,o=0){return kl(e,new Lt(t,16,12),i,r,s,o)}cone(e,t,i,r,s=0,o=0,a=0,l=16){return kl(e,new yr(t,i,l),r,s,o+i/2,a)}rug(e,t,i,r=0,s=0,o=!1){let a=o?new In(e/2,40):new Xn(e,t),l=new $e(a,i);return l.rotation.x=-Math.PI/2,l.position.set(r,.012,s),l.receiveShadow=!0,l.userData.walk=!0,l.userData.outline=!1,this.g.add(l),l}desk(e,t,i=0){let r=this.item(e,t,i),{wood:s,metal:o,screen:a}=this.k;return this.box(r,1,.06,.55,s,0,.5,0),this.box(r,.08,.5,.5,s,-.42,0,0),this.box(r,.08,.5,.5,s,.42,0,0),this.box(r,.3,.02,.18,o,0,.56,-.12),this.box(r,.07,.18,.05,o,0,.58,-.12),this.box(r,.62,.38,.04,a,0,.74,-.14),this.chair(0,.55,Math.PI,r),r}chair(e,t,i=0,r){let s=r?new Oe:this.item(e,t,i);r&&(s.position.set(e,0,t),s.rotation.y=i,r.add(s)),this.box(s,.42,.06,.42,this.k.accent,0,.36,0),this.box(s,.42,.42,.06,this.k.accent,0,.42,-.18);for(let[o,a]of[[-.17,-.17],[.17,-.17],[-.17,.17],[.17,.17]])this.cyl(s,.025,.025,.36,this.k.metal,o,0,a,6);return s}shelf(e,t,i=0,r=1.2,s=1.6){let o=this.item(e,t,i),{darkWood:a,red:l,blue:c,yellow:h,green:d,purple:u}=this.k,p=[l,c,h,d,u];this.box(o,r,s,.4,a,0,0,-.02);let x=Math.max(2,Math.round(s/.42));for(let v=0;v<x;v++){let m=.1+v*(s-.2)/x,f=-r/2+.1;for(;f<r/2-.14;){let b=.06+this.rand()*.06,P=.2+this.rand()*.1;this.box(o,b,P,.26,p[Math.floor(this.rand()*p.length)],f+b/2,m,.06),f+=b+.012}}return o}plant(e,t,i=1){let r=this.item(e,t),{clay:s,leaf:o,leaf2:a}=this.k;return this.cyl(r,.22*i,.16*i,.3*i,s,0,0,0,14),this.ball(r,.34*i,o,0,.5*i,0),this.cone(r,.22*i,.45*i,a,0,.62*i,0,10),r}crate(e,t,i=.5,r=0){let s=this.item(e,t,r);return this.box(s,i,i,i,this.k.crate,0,0,0),this.box(s,i*1.02,i*.12,i*1.02,this.k.darkWood,0,i*.44,0),s}lamp(e,t){let i=this.item(e,t);this.cyl(i,.035,.05,1.7,this.k.metal,0,0,0,8),this.cone(i,.24,.26,this.k.accentSoft,0,1.62,0,16),this.ball(i,.1,this.k.glowWarm,0,1.66,0);let r=new ki(16766874,.5,5,2);r.position.set(0,1.55,0),i.add(r);let s=this.rand()*10;return this.anim.push(o=>r.intensity=.5*(1+.04*Math.sin(o*7.1+s))),i}};function kl(n,e,t,i,r,s){let o=new $e(e,t);return o.position.set(i,r,s),o.castShadow=!0,o.receiveShadow=!0,n.add(o),o}var uf=new Map;function Rr(n,e){let t=uf.get(n);return t||uf.set(n,t=e()),t}function df(n,e){let t=(r,s)=>new ke().setHSL(n/360,r,s,Pt),i=r=>{let s=new ke(r);return Rr(`toon:${s.getHexString()}`,()=>it({color:s}))};return{wood:i(14260827),darkWood:i(9066036),metal:i(9411499),dark:i(4608094),white:i(16513265),paper:i(e.sheet),clay:i(13795157),leaf:i(6270542),leaf2:i(4164163),crate:i(14725996),gold:i(16041282),red:i(15029053),blue:i(5015257),yellow:i(16240980),green:i(5683312),purple:i(10121942),accent:i(t(.55,.55)),accentSoft:i(t(.6,.78)),signal:i(e.signal),ok:i(e.ok),info:i(e.info),screen:it({color:3819096,emissive:e.info,emissiveIntensity:.05}),glowOk:new Ot({color:e.ok,toneMapped:!1}),glowInfo:new Ot({color:e.info,toneMapped:!1}),glowWarm:new Ot({color:16766874,toneMapped:!1}),glowRed:new Ot({color:16730682,toneMapped:!1}),mat:i}}function hf(n,e,t,i,r,s){n.beginPath(),n.moveTo(e+s,t),n.arcTo(e+i,t,e+i,t+r,s),n.arcTo(e+i,t+r,e,t+r,s),n.arcTo(e,t+r,e,t,s),n.arcTo(e,t,e+i,t,s),n.closePath()}function ff(n,e,t){let i=(o,a)=>new ke().setHSL(e/360,o,a,Pt).getStyle(),s=ti(`floor:${n}:${Math.round(e)}`,256,256,1,(o,a,l,c)=>{switch(n){case"planks":case"parquet":{let h=n==="planks"?5:8,d=l/h;for(let u=0;u<h;u++){for(let p=0;p<(n==="planks"?2:4);p++){let x=a/(n==="planks"?2:4),v=n==="planks"?u%2*x*.5:0;o.fillStyle=`hsl(${28+c()*8}, ${50+c()*12}%, ${n==="planks"?56+c()*8:44+c()*10}%)`,o.fillRect(p*x+v-(v?x:0),u*d,x,d),o.fillRect(p*x+v,u*d,x,d),o.fillStyle="rgba(70,38,16,0.7)",o.fillRect(p*x+v,u*d,2,d)}o.fillStyle="rgba(70,38,16,0.75)",o.fillRect(0,u*d,a,2),o.fillStyle="rgba(255,255,255,0.16)",o.fillRect(0,u*d+4,a,4)}break}case"checker":{let d=a/4;for(let u=0;u<4;u++)for(let p=0;p<4;p++)o.fillStyle=(u+p)%2?"#f6f3ec":i(.45,.62),o.fillRect(u*d,p*d,d,d);break}case"hazard":{o.fillStyle="#9aa3ae",o.fillRect(0,0,a,l);for(let h=0;h<6;h++)for(let d=0;d<6;d++)o.fillStyle="rgba(60,66,76,0.35)",o.beginPath(),o.arc(h*44+20,d*44+20,4,0,Math.PI*2),o.fill();o.fillStyle="#f2c230",o.fillRect(0,0,a,26),o.fillStyle="#2b2420";for(let h=-26;h<a;h+=26)o.beginPath(),o.moveTo(h,26),o.lineTo(h+13,0),o.lineTo(h+26,0),o.lineTo(h+13,26),o.fill();break}case"slate":case"marble":{o.fillStyle=n==="slate"?"#4d5866":"#f3efe6",o.fillRect(0,0,a,l);let h=2,d=a/h;for(let u=0;u<h;u++)for(let p=0;p<h;p++){hf(o,u*d+3,p*d+3,d-6,d-6,4),o.fillStyle=n==="slate"?`hsl(214, 14%, ${30+c()*8}%)`:`hsl(40, 30%, ${92+c()*4}%)`,o.fill(),o.strokeStyle=n==="slate"?"rgba(20,24,30,0.6)":"rgba(150,130,100,0.5)",o.lineWidth=2,o.stroke();for(let x=0;x<3;x++)o.strokeStyle=n==="slate"?"rgba(255,255,255,0.08)":i(.25,.75),o.lineWidth=1.5,o.beginPath(),o.moveTo(u*d+c()*d,p*d+c()*d),o.quadraticCurveTo(u*d+c()*d,p*d+c()*d,u*d+c()*d,p*d+c()*d),o.stroke()}break}case"carpet":{o.fillStyle=i(.45,.5),o.fillRect(0,0,a,l),o.strokeStyle=i(.5,.62),o.lineWidth=6;for(let h=0;h<4;h++)for(let d=0;d<4;d++)o.beginPath(),o.arc(h*64+32,d*64+32,16,0,Math.PI*2),o.stroke();break}case"blueprint":{o.fillStyle="#2f6db5",o.fillRect(0,0,a,l),o.strokeStyle="rgba(255,255,255,0.25)",o.lineWidth=1;for(let h=0;h<=a;h+=16)o.beginPath(),o.moveTo(h,0),o.lineTo(h,l),o.stroke(),o.beginPath(),o.moveTo(0,h),o.lineTo(a,h),o.stroke();o.strokeStyle="rgba(255,255,255,0.75)",o.lineWidth=3,o.strokeRect(40,40,110,70),o.beginPath(),o.arc(190,180,36,0,Math.PI*2),o.stroke(),o.beginPath(),o.moveTo(40,200),o.lineTo(130,200),o.lineTo(130,150),o.stroke();break}case"stars":{o.fillStyle="#26305e",o.fillRect(0,0,a,l);for(let h=0;h<70;h++){o.fillStyle=c()<.2?"#ffe9a8":"rgba(255,255,255,0.85)";let d=c()*a,u=c()*l,p=1+c()*2.5;o.beginPath(),o.moveTo(d,u-p*2),o.lineTo(d+p*.6,u),o.lineTo(d,u+p*2),o.lineTo(d-p*.6,u),o.closePath(),o.fill()}break}case"gravel":case"lawn":{o.fillStyle=n==="gravel"?"#c9b99a":"#7fc05a",o.fillRect(0,0,a,l);for(let h=0;h<260;h++)o.fillStyle=n==="gravel"?`hsla(35, 18%, ${55+c()*30}%, 0.9)`:`hsla(${90+c()*20}, 45%, ${40+c()*20}%, 0.6)`,o.beginPath(),o.ellipse(c()*a,c()*l,2+c()*4,1.5+c()*3,c()*3,0,Math.PI*2),o.fill();break}default:{o.fillStyle=i(.28,.66),o.fillRect(0,0,a,l);let h=4,d=a/h;for(let u=0;u<h;u++)for(let p=0;p<h;p++)hf(o,u*d+3,p*d+3,d-6,d-6,6),o.fillStyle=i(.5,(u+p)%2?.84:.8),o.fill()}}}).clone();return s.repeat.set(t,t),s.needsUpdate=!0,s}function pf(n,e){let t=(i,r)=>new ke().setHSL(e/360,i,r,Pt);switch(n){case"panel":return{mat:Rr(`wall:panel:${e}`,()=>it({color:t(.4,.8)})),h:1.1,thick:.14};case"brick":return{mat:Rr("wall:brick",()=>it({color:13133898})),h:1,thick:.2};case"stone":return{mat:Rr("wall:stone",()=>it({color:14275010})),h:.9,thick:.22};case"hedge":return{mat:Rr("wall:hedge",()=>it({color:5216837})),h:.8,thick:.35};case"glass":return{mat:Rr("wall:glass",()=>it({color:12576498,transparent:!0,opacity:.55})),h:1.2,thick:.06};case"fence":return{mat:Rr("wall:fence",()=>it({color:15918799})),h:.6,thick:.08};case"none":return null}}var mf={builder:{name:"Workshop",size:4,shape:"square",floor:"planks",wall:"brick",build(n){let{k:e,s:t}=n,i=n.item(-.6,-t+.75);n.box(i,2.4,.1,.7,e.wood,0,.75,0);for(let l of[-1.1,1.1])n.box(i,.1,.75,.6,e.darkWood,l,0,0);n.box(i,2.2,.06,.5,e.darkWood,0,.25,0),n.box(i,.22,.16,.2,e.metal,-.8,.85,.1),n.box(i,.32,.05,.08,e.red,.2,.85,.1),n.box(i,.05,.05,.36,e.wood,.2,.85,.28),n.box(i,1.6,.9,.05,e.crate,0,1.05,-.3);for(let[l,c,h]of[[-.5,1.5,e.red],[0,1.3,e.blue],[.5,1.55,e.yellow]])n.box(i,.1,.36,.04,h,l,c-.2,-.25);let r=n.item(t-.7,-t+.8,-.3);n.box(r,.6,.3,.35,e.red),n.box(r,.4,.06,.06,e.dark,0,.36,0);let s=n.item(-t+.9,.9,Math.PI/2);for(let l of[-.35,.35])n.box(s,.06,.5,.5,e.darkWood,l,0,0).rotation.x=.2;n.box(s,1.2,.06,.3,e.wood,0,.5,0),n.crate(t-.6,.4,.55),n.crate(t-.55,1.05,.5,.3),n.crate(t-.6,.6,.4).position.y=.55;let o=n.item(t-.6,-.9);n.box(o,.3,.5,.3,e.dark);let a=new Oe;a.position.set(0,1,0),o.add(a),n.cyl(a,.38,.38,.12,e.gold,0,-.06,0,24).rotation.x=Math.PI/2;for(let l=0;l<10;l++){let c=l/10*Math.PI*2,h=n.box(a,.12,.12,.12,e.gold,Math.cos(c)*.44,Math.sin(c)*.44-.06,0);h.rotation.z=c}n.anim.push(l=>a.rotation.z=l*.8),n.desk(-t+1,t-1,Math.PI/2),n.lamp(t-.45,t-.5)}},qa:{name:"Test Lab",size:3.5,shape:"square",floor:"checker",wall:"glass",build(n){let{k:e,s:t}=n,i=n.item(0,-t+.7);n.box(i,2.2,.8,.6,e.white),n.box(i,2.3,.06,.66,e.dark,0,.8,0),[e.green,e.blue,e.purple,e.red,e.yellow].forEach((h,d)=>n.cyl(i,.045,.045,.3,h,-.8+d*.13,.86,0,10)),n.box(i,.75,.05,.12,e.metal,-.55,1,0),n.cyl(i,.06,.2,.3,e.info,.6,.86,0,16),n.cyl(i,.05,.05,.14,e.info,.6,1.16,0,10);let s=[];for(let h=0;h<3;h++)s.push(n.ball(i,.04,e.white,.6,1.3,0));n.anim.push(h=>s.forEach((d,u)=>{let p=(h*.6+u/3)%1;d.position.set(.6+Math.sin(p*9+u)*.04,1.3+p*.5,0),d.scale.setScalar(1-p*.7)}));let o=n.item(t-.5,-.3);n.cyl(o,.05,.05,1.3,e.metal,0,0,0,8),n.box(o,.3,.75,.25,e.dark,0,1.3,0);let a=[e.glowRed,e.glowWarm,e.glowOk].map((h,d)=>n.ball(o,.08,h,0,1.88-d*.22,.13));n.anim.push(h=>a.forEach((d,u)=>d.visible=Math.floor(h/1.5)%3===u));let l=n.item(-t+.8,.2);n.cyl(l,.35,.4,.3,e.metal,0,0,0,20);let c=n.box(l,.25,.25,.25,e.accent,0,.42,0);n.anim.push(h=>c.rotation.y=h);for(let h of[-.35,.35])n.box(l,.06,1,.06,e.dark,h,.3,0);n.box(l,.76,.06,.12,e.dark,0,1.3,0),n.box(l,.5,.02,.06,e.glowOk,0,1.28,0),n.desk(t-1,t-.9,-Math.PI/2),n.plant(-t+.5,t-.5,.8)}}};function So(n,e,t,i,r,s){let o=new $e(e,t);return o.position.set(i,r,s),o.castShadow=!0,o.receiveShadow=!0,n.add(o),o}var gf={release:{name:"Launch Pad",size:4,shape:"square",floor:"hazard",wall:"fence",build(n){let{k:e}=n,t=n.item(-2,-2.2);n.cyl(t,.85,.85,.12,e.dark,0,0,0,32),n.cyl(t,.66,.66,.02,e.signal,0,.12,0,32);let i=new Oe;i.position.y=.2,t.add(i),n.cyl(i,.24,.24,1.5,e.white,0,0,0,20),n.cone(i,.24,.55,e.signal,0,1.5,0,20);for(let a=0;a<3;a++){let l=a/3*Math.PI*2;n.box(i,.18,.5,.04,e.signal,Math.cos(l)*.22,.02,Math.sin(l)*.22).rotation.y=-l}n.ball(i,.11,e.gold,0,.95,.22),n.ball(i,.08,e.info,0,.95,.27),n.anim.push(a=>i.position.y=.2+.04*Math.sin(a*1.3));let r=n.item(-3.3,-2.2);for(let[a,l]of[[-.25,-.25],[.25,-.25],[-.25,.25],[.25,.25]])n.box(r,.06,3.2,.06,e.dark,a,0,l);for(let a of[.4,1.2,2,2.8]){for(let l of[-.25,.25])n.box(r,.5,.05,.05,e.metal,0,a,l);for(let l of[-.25,.25])n.box(r,.05,.05,.5,e.metal,l,a,0)}let s=Math.atan2(.8,.5);for(let a=0;a<3;a++)n.box(r,.94,.04,.04,e.metal,0,.8+a*.8,.25).rotation.z=a%2?-s:s;n.box(r,.8,.06,.12,e.signal,.65,2.2,0);let o=n.ball(r,.09,e.glowRed,0,3.3,0);n.anim.push(a=>o.visible=Math.sin(a*5)>0),n.desk(2.6,1.6,-Math.PI/2);for(let[a,l]of[[-2.5,2.2],[-1.6,2.7],[-3,3.1]]){let c=n.item(a,l);n.cyl(c,.17,.17,.05,e.dark,0,0,0,12),n.cone(c,.13,.42,e.signal,0,.05,0,12),n.cyl(c,.075,.09,.07,e.white,0,.2,0,12)}n.crate(-3.1,.6,.5),n.crate(-3.1,.6,.4).position.y=.5,n.crate(-3.1,1.25,.45,.2),n.crate(3.1,-1.2,.55),n.crate(3.1,-1.2,.42).position.y=.55,n.crate(3.1,-.3,.5,.3)}},"security-reviewer":{name:"Vault",size:3,shape:"square",floor:"slate",wall:"stone",build(n){let{k:e,s:t}=n,i=n.item(0,-t+.25);n.cyl(i,.95,.95,.24,e.mat(8029846),0,1,0,40).rotation.x=Math.PI/2,So(i,new Un(.95,.07,10,40),e.gold,0,1.12,.13);for(let a=0;a<8;a++){let l=a/8*Math.PI*2;n.ball(i,.045,e.dark,Math.cos(l)*.78,1.12+Math.sin(l)*.78,.14)}let r=new Oe;r.position.set(0,1.12,.17),i.add(r),So(r,new Un(.42,.045,8,28),e.gold,0,0,0);for(let a=0;a<3;a++)So(r,new bt(.06,.84,.05),e.gold,0,0,0).rotation.z=a*Math.PI/3;So(r,new Lt(.09,12,10),e.dark,0,0,.03),n.anim.push(a=>r.rotation.z=a*.5);let s=n.item(2.4,-2.4);n.cyl(s,.05,.05,2,e.metal,0,0,0,8);let o=new Oe;o.position.set(0,2,0),s.add(o),n.box(o,.26,.2,.42,e.dark,0,-.1,.1),n.cyl(o,.07,.07,.06,e.info,0,-.03,.32,12).rotation.x=Math.PI/2,n.ball(o,.035,e.glowRed,.09,.12,.2),n.anim.push(a=>o.rotation.y=.9*Math.sin(a*.5));for(let[a,l]of[[-2.3,e.red],[-1.75,e.blue],[-1.2,e.gold]]){let c=n.item(-2.3,a);n.box(c,.5,.36,.42,l,0,0,0),n.box(c,.05,.04,.2,e.dark,.27,.24,0),n.ball(c,.05,e.white,.27,.26,0)}for(let a of[-2.6,2.6]){let l=n.item(a,1.8);n.cyl(l,.05,.06,1,e.metal,0,0,0,8),n.ball(l,.07,e.glowRed,0,1.02,0)}for(let a of[.35,.8])So(n.g,new bt(5.2,.03,.03),e.glowRed,0,a,1.8).userData.walk=!0;n.desk(2.2,.4,-Math.PI/2)}},reviewer:{name:"Review Room",size:3,shape:"square",floor:"carpet",wall:"panel",build(n){let{k:e,s:t}=n,i=n.item(0,-t+.12);n.box(i,2,1.2,.06,e.white,0,.9,0),n.box(i,2.08,.06,.08,e.metal,0,2.1,0),n.box(i,2.1,.05,.1,e.metal,0,.85,.02);for(let a of[-1.02,1.02])n.box(i,.06,1.25,.08,e.metal,a,.9,0);for(let[a,l,c,h,d]of[[-.6,1.6,.5,.15,e.blue],[-.5,1.3,.7,-.05,e.red],[.35,1.7,.4,.4,e.green],[.1,1.1,.9,0,e.blue],[-.25,.95,.4,.2,e.red]])n.box(i,c,.035,.02,d,a,l,.04).rotation.z=h;let r=n.item(-1.9,-1.9);n.box(r,.9,.8,.7,e.darkWood,0,0,0),n.box(r,.7,.06,.5,e.paper,0,.8,0),n.box(r,.68,.06,.48,e.white,0,.86,0);let s=new Oe;s.position.y=.92,r.add(s),n.box(s,.6,.12,.42,e.red,0,0,0),n.cyl(s,.07,.09,.5,e.wood,0,.12,0,12),n.ball(s,.11,e.gold,0,.66,0),n.anim.push(a=>s.position.y=.92+.5*Math.abs(Math.sin(a*1.8)));let o=n.item(1.9,-1);n.box(o,.5,1,.4,e.darkWood,0,0,0),n.box(o,.6,.06,.5,e.wood,0,1,.02).rotation.x=-.35,n.box(o,.4,.03,.3,e.paper,0,1.1,.02),n.chair(1.7,1.9,Math.PI),n.chair(2.3,1.9,Math.PI),n.desk(-2.2,1,Math.PI/2),n.plant(-2.4,2.3,.9)}},architect:{name:"Drafting Studio",size:3.5,shape:"round",floor:"blueprint",wall:"glass",build(n){let{k:e}=n,t=n.item(-1.9,-1.5),i=new Oe;i.position.y=.85,i.rotation.x=.35,t.add(i),n.box(i,1,.05,.7,e.wood,0,-.05,0);for(let c of[-.42,.42])for(let h of[-.25,.25])n.box(t,.06,.82,.06,e.darkWood,c,0,h);n.cyl(t,.03,.03,1.4,e.metal,-.52,0,-.3,8);let r=new Oe;r.position.set(-.52,1.4,-.3),t.add(r),n.box(r,.04,.04,.45,e.metal,0,-.02,.22),n.cone(r,.14,.16,e.signal,0,-.16,.45,16).rotation.x=Math.PI,n.ball(r,.05,e.glowWarm,0,-.1,.45),n.anim.push(c=>r.rotation.y=.15*Math.sin(c*.7)),n.chair(-1.9,-.5,Math.PI);let s=n.item(-2,1.5);for(let c of[-.3,.3])n.box(s,.05,1.6,.05,e.darkWood,c,0,.2).rotation.x=-.2;n.box(s,.7,.9,.04,e.paper,0,.7,-.05),n.box(s,.3,.22,.02,e.purple,-.1,.95,-.03),n.ball(s,.07,e.yellow,.15,1.2,-.03);let o=n.item(1.8,-1.7);n.box(o,.9,.08,.7,e.leaf,0,0,0),n.box(o,.2,.16,.18,e.blue,-.25,.08,-.15),n.box(o,.16,.14,.16,e.white,-.25,.24,-.15),n.box(o,.18,.12,.2,e.yellow,-.2,.08,.15),n.cone(o,.13,.12,e.red,-.2,.2,.15,4),n.cyl(o,.07,.07,.45,e.white,.1,.08,-.1,12),n.cone(o,.1,.2,e.red,.1,.53,-.1,12);let a=new Oe;a.position.set(.22,.08,.14),o.add(a),n.cyl(a,.04,.04,.3,e.gold,0,0,0,8),n.box(a,.36,.03,.03,e.gold,0,.3,0),n.box(a,.02,.1,.02,e.dark,.15,.2,0),n.anim.push(c=>a.rotation.y=c*.7);let l=n.item(2.5,-.7);n.cyl(l,.22,.18,.5,e.mat(6979478),0,0,0,16),n.cyl(l,.04,.04,.42,e.paper,-.08,.3,.02,8).rotation.z=.1,n.cyl(l,.04,.04,.42,e.yellow,.05,.3,-.05,8).rotation.z=-.12,n.cyl(l,.04,.04,.38,e.paper,.02,.3,.08,8).rotation.z=.05,n.desk(2.2,.6,-Math.PI/2)}}};var xf={researcher:{name:"Observatory",size:4,shape:"round",floor:"stars",wall:"stone",build(n){let{k:e}=n,t=n.item(0,-2.4);n.cyl(t,.2,.2,.05,e.dark,0,0,0,16);for(let c=0;c<3;c++){let h=c/3*Math.PI*2,d=new Oe;d.rotation.order="YXZ",d.rotation.y=-h-Math.PI/2,d.rotation.x=.32,d.position.set(0,1.08,0),t.add(d),n.box(d,.05,1.12,.05,e.metal,0,-1.12,0)}let i=new Oe;i.position.set(0,1.08,0),i.rotation.x=.9,t.add(i),n.cyl(i,.12,.09,.9,e.white,0,0,0,16),n.cyl(i,.15,.15,.06,e.gold,0,.9,0,16),n.anim.push(c=>{t.rotation.y=Math.sin(c*.25)*.7,i.rotation.x=.9+Math.sin(c*.4)*.12});let r=n.item(1.5,-2.6);for(let c of[-.5,.5])n.box(r,.06,.4,.06,e.darkWood,c,0,-.04);n.box(r,1.2,.9,.05,e.dark,0,.35,0);let s=[[-.4,1.1],[-.1,1],[.2,1.15],[.4,.95],[-.25,.8],[.1,.75]];for(let[c,h]of s)n.ball(r,.035,e.gold,c,h,.04);n.box(r,.36,.012,.012,e.accentSoft,-.15,.92,.035).rotation.z=.5,n.box(r,.3,.012,.012,e.accentSoft,.3,.85,.035).rotation.z=-.4;for(let c of[-.9,-1.25,-1.6])n.shelf(3*Math.sin(c),-3*Math.cos(c),-c,1.2,1.6);let o=n.item(2.4,.4);n.cyl(o,.3,.3,.06,e.dark,0,0,0,16),n.cyl(o,.04,.04,1.2,e.gold,0,.06,0,8);let a=new $e(new Un(.46,.03,8,32),e.gold);a.position.y=1.3,a.rotation.x=Math.PI/2,a.castShadow=!0,o.add(a);let l=new Oe;l.position.set(0,1.3,0),o.add(l),n.ball(l,.4,e.blue,0,0,0),n.ball(l,.16,e.green,.2,.1,.32),n.ball(l,.13,e.green,-.25,-.12,.25),n.ball(l,.12,e.green,.05,-.2,-.36),n.anim.push(c=>l.rotation.y=c*.6),n.desk(-2,1.9,Math.PI/2),n.lamp(2.1,1.9)}},librarian:{name:"Library",size:4.5,shape:"square",floor:"parquet",wall:"panel",build(n){let{k:e,s:t}=n;for(let l of[-3,-1.5,0,1.5,3])n.shelf(l,-t+.25,0);for(let l of[-2.6,-1.1,.4])n.shelf(-t+.25,l,Math.PI/2),n.shelf(t-.25,l,-Math.PI/2);let i=n.item(-3,.9);for(let l of[-.27,.27]){let c=n.box(i,.06,2.2,.06,e.wood,0,0,l);c.rotation.z=.2}for(let l of[.35,.7,1.05,1.4,1.75])n.box(i,.05,.05,.56,e.wood,-(l-1.1)*Math.tan(.2),l-.025,0);n.rug(2.2,2.2,e.mat(13199962),2.5,2,!0);let r=n.item(2.5,2.2,Math.atan2(-2.5,-2.2));n.box(r,1,.36,.9,e.accent,0,.14,0),n.box(r,.86,.12,.8,e.accentSoft,0,.5,.02),n.box(r,1,.8,.22,e.accent,0,.14,-.34);for(let l of[-.41,.41])n.box(r,.18,.38,.9,e.accent,l,.14,0);for(let l of[-.4,.4])for(let c of[-.36,.36])n.box(r,.1,.14,.1,e.dark,l,0,c);n.lamp(3.6,1);let s=n.item(2,3.4);n.box(s,.7,.04,.45,e.wood,0,.4,0),n.box(s,.7,.04,.45,e.wood,0,.9,0);for(let l of[-.32,.32])for(let c of[-.2,.2])n.box(s,.04,.9,.04,e.metal,l,0,c);n.box(s,.1,.22,.3,e.red,-.2,.44,0),n.box(s,.1,.22,.3,e.blue,-.08,.44,0),n.box(s,.1,.26,.3,e.green,-.2,.94,0),n.box(s,.1,.22,.3,e.yellow,-.08,.94,0);let o=[];for(let l of[-.32,.32])for(let c of[-.2,.2]){let h=new Oe;h.position.set(l,.07,c),s.add(h),n.cyl(h,.07,.07,.04,e.dark,0,-.02,0,10).rotation.z=Math.PI/2,o.push(h)}n.anim.push(l=>o.forEach(c=>c.rotation.x=l*3));let a=n.item(-2.8,2.6);n.box(a,1,1.5,.5,e.darkWood,0,0,0),n.box(a,1.04,.05,.54,e.wood,0,1.5,0);for(let l=0;l<5;l++)for(let c=0;c<3;c++)n.box(a,.26,.2,.03,e.crate,-.3+c*.3,.1+l*.27,.265),n.ball(a,.025,e.gold,-.3+c*.3,.2+l*.27,.29);n.desk(-1.6,3.6,Math.PI)}},planner:{name:"Planning Room",size:3.5,shape:"round",floor:"carpet",wall:"hedge",build(n){let{k:e}=n,t=n.item(0,-2.5);for(let p of[-.9,.9])n.cyl(t,.04,.04,2.1,e.dark,p,0,-.1,8);n.box(t,2.1,1.2,.06,e.white,0,.9,0);let i=[-.62,0,.62],r=[e.red,e.yellow,e.green];i.forEach((p,x)=>n.box(t,.55,.08,.03,r[x],p,1.9,.04));let s=[[-.62,1.68,e.yellow],[-.62,1.44,e.purple],[-.62,1.2,e.blue],[0,1.68,e.green],[0,1.44,e.gold],[.62,1.68,e.red],[.62,1.44,e.blue],[.62,1.2,e.purple]];for(let[p,x,v]of s)n.box(t,.42,.18,.02,v,p,x,.05);let o=n.box(t,.42,.18,.02,e.gold,-.62,.96,.06),a=[-.62,0,.62,0];n.anim.push(p=>{let x=p*.2%4,v=Math.floor(x),m=x-v,f=a[v],b=a[(v+1)%4];o.position.x=f+(b-f)*m*m*(3-2*m)});let l=n.item(2.75,.2,-Math.PI/2);n.box(l,.5,.04,.3,e.dark,0,0,-.05),n.cyl(l,.03,.03,1,e.dark,0,0,-.05,8),n.box(l,1,1.2,.06,e.wood,0,1,0),n.box(l,.88,1.08,.03,e.paper,0,1.06,.04);for(let p=0;p<4;p++)for(let x=0;x<5;x++)n.box(l,.15,.2,.01,e.white,-.36+x*.18,1.12+p*.25,.06);let c=[[0,1,e.red],[2,3,e.blue],[4,0,e.green],[1,2,e.yellow]];for(let[p,x,v]of c)n.box(l,.11,.06,.01,v,-.36+p*.18,1.12+x*.25+.07,.08);let h=n.item(-1.9,-1.25);n.cyl(h,.42,.42,.05,e.wood,0,.72,0,20),n.cyl(h,.07,.2,.7,e.darkWood,0,0,0,12),n.chair(-1.9,-1.95,0),n.chair(-2.55,-1.25,Math.PI/2),n.chair(-1.9,-.35,Math.PI);let d=n.item(1.9,-1.6);n.cyl(d,.3,.3,.08,e.dark,0,0,0,16),n.cyl(d,.04,.04,2.4,e.metal,0,0,0,8),n.ball(d,.07,e.gold,0,2.45,0);let u=[[.9,e.red],[1.4,e.blue],[1.9,e.green]];for(let[p,x]of u){let v=new Oe;v.position.set(0,p,0),d.add(v),n.box(v,.36,.2,.02,x,.2,-.1,0),n.anim.push(m=>v.rotation.y=Math.sin(m*1.5+p*2)*.35)}n.desk(-2.2,1,Math.PI/2)}},auditor:{name:"Ledger Office",size:3,shape:"square",floor:"tiles",wall:"brick",build(n){let{k:e}=n,t=e.mat(11121860);for(let c of[-2.1,-1.5,-.9,-.3]){let h=n.item(c,-2.5);n.box(h,.6,1.25,.5,e.metal,0,0,0);for(let d=0;d<4;d++){let u=.06+d*.29;n.box(h,.54,.25,.03,t,0,u,.265),n.box(h,.18,.03,.04,e.dark,0,u+.17,.29)}}let i=n.desk(2.3,-.9,-Math.PI/2);n.box(i,.24,.1,.2,e.paper,.25,.56,.1),n.box(i,.2,.14,.18,e.white,-.3,.56,.12),n.box(i,.22,.03,.16,e.blue,.28,.66,.1);let r=n.item(-1.9,.6);n.cyl(r,.35,.35,.08,e.dark,0,0,0,16),n.cyl(r,.05,.05,1.42,e.metal,0,.08,0,8);let s=new Oe;s.position.set(0,1.5,0),r.add(s);let o=new $e(new Un(.5,.06,12,32),e.gold);o.castShadow=!0,s.add(o),n.cyl(s,.47,.47,.02,e.mat(12576498),0,-.01,0,32).rotation.x=Math.PI/2;let a=n.box(s,.1,.7,.1,e.darkWood,0,0,0);a.position.set(-.6,-.6,0),a.rotation.z=3*Math.PI/4,n.anim.push(c=>s.rotation.x=Math.sin(c*1.1)*.22);let l=n.item(1.9,1.6);n.box(l,.8,.05,.5,e.wood,0,.7,0);for(let c of[-.34,.34])for(let h of[-.2,.2])n.box(l,.05,.7,.05,e.darkWood,c,0,h);n.box(l,.5,.04,.08,e.darkWood,0,.75,0);for(let c of[-.25,.25])n.box(l,.04,.38,.08,e.darkWood,c,.79,0);n.box(l,.5,.04,.08,e.darkWood,0,1.13,0);for(let c of[.87,.95,1.03,1.11]){let h=n.cyl(l,.008,.008,.5,e.metal,0,0,0,6);h.position.set(0,c,0),h.rotation.z=Math.PI/2;for(let d=0;d<5;d++)n.ball(l,.028,d%2?e.red:e.yellow,-.2+d*.1,c,.02)}n.plant(2.3,2.3,.9)}}};var vf={finance:{name:"Counting House",size:3.5,shape:"square",floor:"marble",wall:"stone",build(n){let{k:e}=n,t=n.item(-2.5,-1.8);for(let c=0;c<6;c++)n.cyl(t,.16,.16,.05,e.gold,0,c*.05,0,20);let i=n.cyl(t,.16,.16,.05,e.gold,0,.4,0,20);n.anim.push(c=>{let h=c*.7%1;i.position.y=.4+Math.sin(h*Math.PI)*.25,i.rotation.x=h*Math.PI*2});let r=n.item(-2,-2.7);for(let c=0;c<3;c++)n.cyl(r,.16,.16,.05,e.gold,0,c*.05,0,20);let s=n.item(2.3,-1.9);for(let c of[-.13,.13]){for(let h of[-.42,0,.42])n.box(s,.4,.14,.22,e.gold,h,0,c);for(let h of[-.21,.21])n.box(s,.4,.14,.22,e.gold,h,.14,c)}n.box(s,.4,.14,.22,e.gold,0,.28,0);let o=n.item(2.4,-2.9);n.box(o,.9,.9,.6,e.dark,0,0,0),n.box(o,.7,.7,.03,e.metal,0,.1,.31);let a=new Oe;a.position.set(0,.45,.35),o.add(a),n.cyl(a,.12,.12,.04,e.gold,0,0,0,16).rotation.x=Math.PI/2,n.anim.push(c=>a.rotation.z=c*.9);let l=n.item(0,-2.85);n.box(l,2.4,.9,.5,e.darkWood,0,0,0),n.box(l,2.5,.08,.6,e.wood,0,.9,0),n.box(l,2.3,.06,.05,e.metal,0,1.5,.26);for(let c=0;c<=11;c++)n.box(l,.04,.52,.04,e.metal,-1.1+c*.2,.98,.26);n.desk(-2.3,1.2,0),n.plant(2.5,2.4,.9)}},incident:{name:"Ops Room",size:3.5,shape:"square",floor:"hazard",wall:"panel",build(n){let{k:e}=n,t=n.item(0,-3.3);n.box(t,2.6,1.2,.12,e.dark,0,.7,0);for(let l of[-1.1,1.1])n.box(t,.1,.7,.1,e.metal,l,0,0);for(let l of[.9,1.4])for(let c of[-.82,0,.82])n.box(t,.7,.4,.03,e.screen,c,l,.075);let i=n.item(2.4,-2.4);n.box(i,.5,.08,.5,e.dark,0,0,0),n.cyl(i,.04,.04,.9,e.metal,0,.08,0,8);let r=new Oe;r.position.set(0,.95,0),i.add(r),n.ball(r,.22,e.red,0,0,0).scale.set(1,.6,1),n.box(r,.1,.1,.1,e.glowRed,.2,-.02,0);let s=new ki(16730682,.8,4,2);s.position.set(0,.05,0),r.add(s),n.anim.push(l=>{r.rotation.y=l*3,s.intensity=.2+.8*Math.max(0,Math.sin(l*3))});for(let[l,c]of[[-2.8,-2.7],[-2.3,-2.8]]){let h=n.item(l,c);n.cyl(h,.1,.1,.5,e.red,0,0,0,14),n.cyl(h,.105,.105,.08,e.white,0,.2,0,14),n.cyl(h,.04,.05,.08,e.dark,0,.5,0,8),n.box(h,.04,.22,.04,e.dark,.13,.25,0)}let o=n.desk(-2.2,1,0);n.box(o,.22,.06,.16,e.red,.3,.56,.1),n.box(o,.26,.04,.05,e.red,.3,.62,.1);let a=n.item(-3.3,0,Math.PI/2);n.box(a,1.5,1,.04,e.metal,0,.8,-.02),n.box(a,1.4,.9,.05,e.white,0,.85,0),n.box(a,.5,.04,.02,e.red,-.25,1.3,.03),n.box(a,.8,.04,.02,e.blue,.1,1.1,.03),n.box(a,.4,.04,.02,e.green,-.3,.95,.03),n.plant(2.6,2.6,.9)}},"edge-operator":{name:"Antenna Yard",size:4,shape:"round",floor:"gravel",wall:"fence",build(n){let{k:e}=n,t=n.item(-2,-1.7),i=[[-1,-1],[1,-1],[1,1],[-1,1]],r=d=>d*.5,s=d=>.3-d*.035,o=(d,u)=>{let[p,x]=i[u];return[p*s(d),r(d),x*s(d)]},a=(d,u)=>{let p=new G(u[0]-d[0],u[1]-d[1],u[2]-d[2]),x=p.length();n.box(t,.035,x,.035,e.metal,(d[0]+u[0])/2,(d[1]+u[1])/2-x/2,(d[2]+u[2])/2).quaternion.setFromUnitVectors(new G(0,1,0),p.normalize())};for(let d=0;d<6;d++)for(let u=0;u<4;u++)a(o(d,u),o(d,(u+1)%4)),d<5&&(a(o(d,u),o(d+1,u)),a(o(d,u),o(d+1,(u+1)%4)));let l=new Oe;l.position.set(0,2.5,0),t.add(l),n.cyl(l,.5,.08,.16,e.white,0,-.08,.15,24).rotation.x=Math.PI/2,n.box(l,.05,.05,.2,e.metal,0,-.025,.07),n.anim.push(d=>l.rotation.y=d*.5);let c=n.ball(t,.07,e.glowRed,0,3.1,0);n.anim.push(d=>c.visible=Math.sin(d*4)>0);for(let[d,u]of[[2.2,1.2],[1.5,2.3]]){let p=n.item(d,u),x=n.cyl(p,.2,.2,.4,e.yellow,0,0,0,16);x.rotation.z=Math.PI/2,x.position.set(0,.3,0);for(let v of[-.22,.22]){let m=n.cyl(p,.3,.3,.04,e.dark,0,0,0,16);m.rotation.z=Math.PI/2,m.position.set(v,.3,0)}}for(let[d,u]of[[2.3,-1.6],[2.3,-.3]]){let p=n.item(d,u);n.cyl(p,.03,.03,.6,e.metal,-.35,0,0,6),n.cyl(p,.03,.03,.6,e.metal,.35,0,0,6);let x=new Oe;x.position.set(0,.6,0),x.rotation.x=.5,p.add(x),n.box(x,.8,.04,.5,e.blue,0,-.02,0),n.box(x,.02,.05,.5,e.white,0,-.02,0)}let h=n.item(-2.1,1.3);n.box(h,.8,.7,.8,e.white,0,0,0),n.cone(h,.65,.5,e.red,0,.7,0,4).rotation.y=Math.PI/4,n.box(h,.25,.42,.02,e.darkWood,0,0,.41),n.box(h,.22,.2,.02,e.glowWarm,-.2,.42,.41),n.desk(0,2.5,Math.PI)}},mayor:{name:"Town Hall",size:4.5,shape:"square",floor:"marble",wall:"stone",build(n){let{k:e}=n;for(let o of[-3.2,-1.6,0,1.6,3.2]){let a=n.item(o,-3.9);n.box(a,.6,.12,.6,e.white,0,0,0),n.cyl(a,.2,.22,2,e.white,0,.12,0,16),n.box(a,.6,.14,.6,e.white,0,2.12,0)}let t=n.item(0,-2.7);n.box(t,.9,1,.5,e.darkWood,0,0,0),n.box(t,1,.08,.6,e.wood,0,1,0),n.box(t,.5,.4,.02,e.gold,0,.3,.26),n.cyl(t,.015,.015,.4,e.metal,.25,1.08,.05,6),n.ball(t,.06,e.dark,.25,1.52,.05);let i=n.item(-3.6,-2.4);n.cyl(i,.03,.03,2.6,e.metal,0,0,0,8),n.ball(i,.06,e.gold,0,2.62,0);let r=new Oe;r.position.set(0,2.22,0),i.add(r);let s=n.box(r,.7,.45,.02,e.blue,.35,-.225,0);n.anim.push(o=>{r.rotation.y=Math.sin(o*3)*.3,s.rotation.z=Math.sin(o*6)*.1,s.scale.y=1+.08*Math.sin(o*6)}),n.rug(1.2,6.6,e.red,0,.9);for(let[o,a]of[[-3.6,Math.PI/2],[3.6,-Math.PI/2]]){let l=n.item(o,.6,a);for(let c of[-.7,.7])n.box(l,.08,.42,.45,e.darkWood,c,0,0);n.box(l,1.6,.08,.5,e.wood,0,.42,0),n.box(l,1.6,.42,.06,e.wood,0,.5,-.22)}n.desk(2.4,2.4,Math.PI),n.plant(-3.3,2.9,1)}},"mayor-office":{name:"Mayor's Office",size:3,shape:"square",floor:"carpet",wall:"panel",build(n){let{k:e}=n,t=n.item(0,-2.3);n.box(t,2,.07,.8,e.wood,0,.72,0);for(let o of[-.65,.65])n.box(t,.5,.72,.7,e.darkWood,o,0,0);n.box(t,.28,.02,.18,e.metal,0,.79,-.25),n.box(t,.07,.14,.05,e.metal,0,.81,-.25),n.box(t,.6,.4,.04,e.screen,0,.95,-.25);let i=n.item(-2.1,-2.1,Math.PI/4);n.box(i,.6,.05,.6,e.darkWood,0,0,0),n.box(i,.7,.3,.7,e.accent,0,.05,0),n.box(i,.12,.45,.7,e.accent,-.29,.05,0),n.box(i,.12,.45,.7,e.accent,.29,.05,0),n.box(i,.7,1.1,.14,e.accent,0,.35,-.28),n.shelf(2.75,-.9,-Math.PI/2,1.2,1.6);let r=n.item(2.3,-2.5);n.cyl(r,.12,.12,.05,e.gold,0,0,0,16),n.cyl(r,.03,.03,2.2,e.metal,0,0,0,8),n.box(r,.5,.36,.02,e.blue,.27,1.7,0),n.ball(r,.05,e.gold,0,2.23,0);let s=n.item(0,-2.94);n.box(s,.6,.7,.04,e.gold,0,1.45,0),n.box(s,.5,.6,.02,e.accentSoft,0,1.5,.02),n.box(s,.3,.18,.03,e.blue,0,1.45,.05),n.ball(s,.1,e.clay,0,1.68,.05),n.rug(1.8,1.8,e.purple,0,.9,!0),n.plant(-2.4,2.2,.9)}}};var B1={name:"Studio",size:3,shape:"square",floor:"tiles",wall:"panel",build(n){let e=n.s;n.desk(-e+1.2,-e+1),n.desk(e-1.2,-e+1),n.plant(-e+.5,e-.6,.9),n.plant(e-.5,e-.6,.9),n.lamp(e-.45,0)}},H1={...mf,...gf,...xf,...vf};function wo(n){return H1[n]??B1}function yf(n,e,t,i){let r=wo(n),s=new Oe,o=df(t,i),a=new Ll(s,e,t,o,Ui(t*131+n.length));z1(a,r),G1(a,r.name,r.shape==="round"),r.build(a),ei(s);let l=a.anim,{glowOk:c,glowInfo:h}=o;return{group:s,floor:it({map:ff(r.floor,t,e)}),screens:[o.screen],animate:d=>{c.color.copy(i.ok).multiplyScalar(.4+.6*Math.max(0,Math.sin(d*2.7))),h.color.copy(i.info).multiplyScalar(.4+.6*Math.max(0,Math.sin(d*3.9+1)));for(let u of l)u(d)}}}function z1(n,e){let t=pf(e.wall,n.hue);if(!t)return;let{mat:i,h:r,thick:s}=t,o=n.s,a=(l,c,h,d)=>{let u=n.item(l,c,d);n.box(u,h,r,s,i),e.wall!=="hedge"&&e.wall!=="glass"&&n.box(u,h+.04,.06,s+.06,n.k.darkWood,0,r,0)};if(e.shape==="round"){let c=o-.15;for(let h=0;h<9;h++){let d=Math.PI+(h+.5)*(Math.PI/9),u=2*c*Math.sin(Math.PI/18)+.05;a(Math.cos(d)*c,Math.sin(d)*c,u,-d-Math.PI/2)}}else{a(0,-o+s/2+.05,2*o-.1,0);for(let l of[-1,1]){let c=o*.9;a(l*(o-s/2-.05),-o+c/2+.05,c,Math.PI/2)}}}function G1(n,e,t){let i=ti(`roomsign:${e}`,512,128,1,(a,l,c)=>{a.fillStyle="#fff6e0",a.fillRect(0,0,l,c),a.strokeStyle="#2b2420",a.lineWidth=10,a.strokeRect(5,5,l-10,c-10),a.fillStyle="#2b2420",a.font='700 64px ui-rounded, "Comic Sans MS", system-ui, sans-serif',a.textAlign="center",a.textBaseline="middle",a.fillText(e,l/2,c/2+4)}),r=it({map:i}),s=n.item(0,-n.s+(t?.45:.3));n.cyl(s,.04,.04,1.5,n.k.darkWood,-.8,0,0,8),n.cyl(s,.04,.04,1.5,n.k.darkWood,.8,0,0,8);let o=new $e(new bt(1.9,.48,.06),[n.k.wood,n.k.wood,n.k.wood,n.k.wood,r,n.k.wood]);o.position.set(0,1.42,.02),o.castShadow=!0,s.add(o)}var V1=64,W1=32,X1=4e3,q1=1500,$1=1.6,tn=10,_f=24,Y1=60,bf=2,Z1=.9,K1=[0,30,-30,60,-60];function Dl(n){return 2*(Math.max(1,Math.ceil(Math.sqrt(n.length)))+1)}function J1(n){let e=0;for(let t of n)e=e*31+t.charCodeAt(0)>>>0;return e%360}function Mf(n,e,t=0){return[(n-e)*(V1/2),(n+e)*(W1/2)-t]}function j1(n){let e=new Map;for(let h of n){let d=h.tier==="orchestrator"?"":h.role;e.set(d,[...e.get(d)??[],h])}let t=[],i=new Map,r=(h,d)=>{t.push(h);let u=Math.max(1,Math.ceil(Math.sqrt(d.length)));d.sort((p,x)=>p.cid-x.cid).forEach((p,x)=>{let v=h.cx-(u-1)*2+x%u*4,m=h.cy-(u-1)*2+Math.floor(x/u)*4;i.set(p.cid,[h,v,m])})},s=e.get("");s&&r({role:"orchestrator",cx:0,cy:0,r:Dl(s),hue:25},s);let o=[...e.keys()].filter(h=>h).sort(),a=Math.max(0,...o.map(h=>Dl(e.get(h)))),l=s?Dl(s):0,c=Math.max(o.length>1?(2*a*Math.SQRT2+bf)/(2*Math.sin(Math.PI/o.length)):0,(l+a)*Math.SQRT2+bf);return o.forEach((h,d)=>{let u=e.get(h),p=d/o.length*Math.PI*2-Math.PI/2;r({role:h,cx:Math.round(Math.cos(p)*c),cy:Math.round(Math.sin(p)*c),r:Dl(u),hue:J1(h)},u)}),{districts:t,homes:i}}function Q1(){let n=getComputedStyle(document.documentElement),e=(t,i)=>n.getPropertyValue(`--${t}`).trim()||i;return{paper:e("paper","#f5f3ee"),sheet:e("sheet","#ffffff"),rule:e("rule","#d6d2c8"),ink:e("ink","#1a1a1a"),ink2:e("ink-2","#4a4a4a"),ink3:e("ink-3","#8a8a8a"),signal:e("signal","#d9482b"),ok:e("ok","#2f8f5b"),info:e("info","#2f6fb0"),danger:e("danger","#c0392b")}}function Fi(n){return typeof n=="string"?n:""}function Cr(n,e,t){return Math.min(t,Math.max(e,n))}var ey=[[2.6,2.6],[-2.6,-2.6],[2.6,-2.6],[-2.6,2.6]],ty={builder:"rack",qa:"rack",release:"rack",security:"rack",researcher:"shelf",librarian:"shelf",planner:"shelf",architect:"board",reviewer:"board"};function ny(n){let e=ey.map(([t,i])=>({kind:"bench",x:t,y:i}));for(let t of n){if(t.role==="orchestrator")continue;let i=t.r-.7;e.push({kind:"plant",x:t.cx-i,y:t.cy-i,d:t},{kind:"desk",x:t.cx+i,y:t.cy-i,d:t},{kind:"desk",x:t.cx-i,y:t.cy+i,d:t},{kind:"lamp",x:t.cx+i,y:t.cy+i,d:t},{kind:"crate",x:t.cx-i,y:t.cy,d:t},{kind:ty[t.role]??"crate",x:t.cx,y:t.cy-i,d:t}),t.r>=4&&e.push({kind:"desk",x:t.cx+i,y:t.cy,d:t})}return e}function iy(n,e,t,i){return i.some(r=>r!==n&&Math.hypot(r.x-e,r.y-t)<Z1)}function ry(n,e){let t=e.filter(i=>i!==n&&i.district===n.district&&i.agent.status==="idle"&&Math.hypot(i.x-n.x,i.y-n.y)<8);return t[Math.floor(Math.random()*t.length)]??null}function Sf(n){let e=y("canvas.floor-canvas",{"aria-label":"The plaza floor"}),t=y("div.floor-card",{hidden:!0}),i=y("div.floor-legend",null,y("span",null,y("i.floor-key.working"),"Working"),y("span",null,y("i.floor-key.idle"),"Idle"),y("span",null,y("i.floor-key.offline"),"Offline")),r=y("div.floor-stage",null,e,t,i);n.classList.add("floor-view"),n.append(y("div.view-head",null,y("h1",null,"Floor"),y("div.actions",null,Ze("Reset view",()=>A(),{kind:"ghost",icon:"refresh"}))),r);let s=e.getContext("2d"),o=[],a=[],l=[],c=new Map,h=[],d={x:0,y:0},u=1,p=0,x=0,v=1,m=null,f=null,b="",P=null,w=!1,T=!0,E=0,D=performance.now(),g=Q1();function S(C){let L=j1(C);o=L.districts;let O=[];for(let H of C){let q=L.homes.get(H.cid);if(!q)continue;let[K,re,pe]=q,me=c.get(H.cid);O.push(me?Object.assign(me,{agent:H,district:K,home:[re,pe]}):{agent:H,district:K,home:[re,pe],x:re,y:pe,tx:re,ty:pe,facing:1,walk:0,moving:!1,nextWander:performance.now()+Math.random()*3e3,bubble:null,activity:"idle",talk:null,talkUntil:0})}a=O,l=ny(o),c.clear(),a.forEach(H=>c.set(H.agent.cid,H)),f&&(f=c.get(f.agent.cid)??null),m&&(m=c.get(m.agent.cid)??null),W()}function _(C,L,O=0){let H=p/2+d.x,q=x/2+d.y,[K,re]=Mf(C,L,O);return[H+K*u,q+re*u]}function A(){if(!o.length){d={x:0,y:0},u=1;return}let C=1/0,L=-1/0,O=1/0,H=-1/0;for(let re of o){let pe=[[re.cx-re.r,re.cy-re.r],[re.cx+re.r,re.cy-re.r],[re.cx+re.r,re.cy+re.r],[re.cx-re.r,re.cy+re.r]];for(let[me,B]of pe){let[ue,se]=Mf(me,B);C=Math.min(C,ue),L=Math.max(L,ue),O=Math.min(O,se-60),H=Math.max(H,se+tn)}}let q=Math.max(1,L-C),K=Math.max(1,H-O);u=Cr(Math.min(p*.85/q,x*.85/K),.4,2.5),d={x:-((C+L)/2)*u,y:-((O+H)/2)*u}}function I(C,L){for(let O of a){let H=O.agent.status==="idle";if(H||(O.talk=null),H&&L>=O.nextWander){let Rt=Math.random()<.3?ry(O,a):null;if(Rt)O.talk=Rt.agent.cid,O.talkUntil=L+9e3,O.nextWander=L+9e3;else{let an=Math.max(.5,O.district.r-.7);O.tx=O.district.cx+(Math.random()*2-1)*an,O.ty=O.district.cy+(Math.random()*2-1)*an,O.nextWander=L+3e3+Math.random()*4e3}}let q=H&&O.talk!==null&&L<O.talkUntil?c.get(O.talk):void 0;O.talk!==null&&!q&&(O.talk=null);let[K,re]=O.home,pe=q?[q.x+.9,q.y+.9]:H?[O.tx,O.ty]:[K,re],[me,B]=pe,ue=me-O.x,se=B-O.y,ge=Math.hypot(ue,se),ye=Math.min($1*C,ge),de=Math.atan2(se,ue),Pe=K1.map(Rt=>de+Rt*Math.PI/180).find(Rt=>!iy(O,O.x+Math.cos(Rt)*ye,O.y+Math.sin(Rt)*ye,a));O.moving=ge>.02&&Pe!==void 0,Pe!==void 0&&(O.x+=Math.cos(Pe)*ye,O.y+=Math.sin(Pe)*ye);let{cx:Le,cy:vt,r:ot}=O.district;if(O.x=Cr(O.x,Le-ot+.5,Le+ot-.5),O.y=Cr(O.y,vt-ot+.5,vt+ot-.5),O.moving){O.walk+=C*9;let Rt=ue-se;Math.abs(Rt)>.05&&(O.facing=Rt>0?1:-1)}q&&!O.moving&&(O.facing=q.x-O.x-(q.y-O.y)>=0?1:-1,O.bubble={text:"\xB7 \xB7 \xB7",until:L+3e3,tone:"info"},O.talk=null,O.nextWander=L+2500+Math.random()*2e3)}}function k(C,L){s.beginPath(),C.forEach(([O,H],q)=>q===0?s.moveTo(O,H):s.lineTo(O,H)),s.closePath(),s.fillStyle=L,s.fill()}function N(C,L,O,H,q){s.beginPath(),s.moveTo(C+q,L),s.arcTo(C+O,L,C+O,L+H,q),s.arcTo(C+O,L+H,C,L+H,q),s.arcTo(C,L+H,C,L,q),s.arcTo(C,L,C+O,L,q),s.closePath()}function U(C){let L=C.cx-C.r,O=C.cx+C.r,H=C.cy-C.r,q=C.cy+C.r,K=(de,Pe)=>_(de,Pe,tn),re=(de,Pe)=>_(de,Pe,0);k([K(O,H),K(O,q),re(O,q),re(O,H)],`hsl(${C.hue} 30% 32% / 0.5)`),k([K(O,q),K(L,q),re(L,q),re(O,q)],`hsl(${C.hue} 30% 22% / 0.6)`);let pe=[K(L,H),K(O,H),K(O,q),K(L,q)];k(pe,g.sheet),k(pe,`hsl(${C.hue} 30% 50% / 0.12)`);let me=C.role==="orchestrator";s.beginPath(),pe.forEach(([de,Pe],Le)=>Le===0?s.moveTo(de,Pe):s.lineTo(de,Pe)),s.closePath(),s.strokeStyle=me?g.signal:g.rule,s.lineWidth=me?2:1,s.stroke();let[B,ue]=K(L,H),se=C.role.toUpperCase();s.font="10px ui-monospace, SFMono-Regular, Menlo, monospace";let ge=s.measureText(se).width+12,ye=ue+10;N(B-ge/2,ye,ge,16,3),s.fillStyle=g.sheet,s.fill(),s.strokeStyle=me?g.signal:g.rule,s.lineWidth=1,s.stroke(),s.fillStyle=g.ink2,s.textAlign="center",s.textBaseline="middle",s.fillText(se,B,ye+8.5)}function z(C,L,O){let K=(ge,ye)=>_(ge,ye,tn+12),re=(ge,ye)=>_(ge,ye,tn),pe=[C-.32,L-.32],me=[C+.32,L-.32],B=[C+.32,L+.32],ue=[C-.32,L+.32];k([K(me[0],me[1]),K(B[0],B[1]),re(B[0],B[1]),re(me[0],me[1])],g.ink3),k([K(B[0],B[1]),K(ue[0],ue[1]),re(ue[0],ue[1]),re(B[0],B[1])],g.ink2),k([K(pe[0],pe[1]),K(me[0],me[1]),K(B[0],B[1]),K(ue[0],ue[1])],g.sheet);let se=[K(C-.2,L-.2),K(C+.2,L-.2),K(C+.2,L+.2),K(C-.2,L+.2)];s.save(),s.globalAlpha=O?1:.6,k(se,O?g.info:g.rule),s.restore()}function Y(){s.save(),s.lineCap="round";for(let H of o){if(H.role==="orchestrator")continue;let[q,K]=_(0,0),[re,pe]=_(H.cx,H.cy);s.beginPath(),s.moveTo(q,K),s.lineTo(re,pe),s.globalAlpha=.8,s.strokeStyle=g.rule,s.lineWidth=11*u,s.stroke(),s.globalAlpha=.9,s.strokeStyle=g.sheet,s.lineWidth=6*u,s.stroke()}s.restore();let C=3.5,L=[_(-C,-C),_(C,-C),_(C,C),_(-C,C)];k(L,g.sheet),s.save(),s.strokeStyle=g.rule,s.lineWidth=1,s.beginPath(),L.forEach(([H,q],K)=>K===0?s.moveTo(H,q):s.lineTo(H,q)),s.closePath(),s.stroke();let O=2.4;s.globalAlpha=.6,s.setLineDash([4,4]),s.beginPath(),s.moveTo(..._(-O,-O)),s.lineTo(..._(O,-O)),s.lineTo(..._(O,O)),s.lineTo(..._(-O,O)),s.closePath(),s.stroke(),s.restore()}function ee(C,L){let[O,H]=_(C,L,tn);s.save(),s.translate(O,H),s.scale(1,.5);let q=s.createRadialGradient(0,0,0,0,0,36*u);q.addColorStop(0,"rgba(255, 214, 140, 0.5)"),q.addColorStop(1,"rgba(255, 214, 140, 0)"),s.fillStyle=q,s.beginPath(),s.arc(0,0,36*u,0,Math.PI*2),s.fill(),s.restore()}function J(C,L,O,H,q,K,re,pe){let me=(se,ge,ye)=>_(se,ge,ye),B=H,ue=H+q;k([me(C+O,L-O,ue),me(C+O,L+O,ue),me(C+O,L+O,B),me(C+O,L-O,B)],pe),k([me(C+O,L+O,ue),me(C-O,L+O,ue),me(C-O,L+O,B),me(C+O,L+O,B)],re),k([me(C-O,L-O,ue),me(C+O,L-O,ue),me(C+O,L+O,ue),me(C-O,L+O,ue)],K)}function ie(C,L,O,H){let q=tn;J(C,L,.42,q,9,"hsl(32 30% 74%)","hsl(32 25% 52%)","hsl(32 25% 60%)");let K=C,re=L-.12,pe=.12;J(K,re,pe,q+9,11,g.ink2,g.ink,g.ink2);let me=(B,ue)=>_(B,re+pe,ue);s.save(),O&&(s.shadowColor=g.info,s.shadowBlur=8+4*Math.sin(H/400)),k([me(K-pe*.8,q+11),me(K+pe*.8,q+11),me(K+pe*.8,q+18),me(K-pe*.8,q+18)],O?g.info:g.rule),s.restore()}function ce(C,L){let O=tn;J(C,L,.2,O,6,"hsl(18 35% 52%)","hsl(18 30% 38%)","hsl(18 30% 44%)");let[H,q]=_(C,L,O+6),K=[[-4,-9,-.6],[4,-9,.6],[0,-13,0],[-2,-6,-1.2],[3,-6,1.1]];s.save(),s.fillStyle="hsl(118 38% 40%)";for(let[re,pe,me]of K)s.beginPath(),s.ellipse(H+re*u,q+pe*u,5*u,2.8*u,me,0,Math.PI*2),s.fill();s.restore()}function Se(C,L){J(C,L,.22,tn,10,"hsl(35 45% 64%)","hsl(35 40% 42%)","hsl(35 40% 50%)")}function fe(C,L){let[O,H]=_(C,L,tn),[q,K]=_(C,L,tn+44);s.save(),s.strokeStyle=g.ink2,s.lineWidth=1.5,s.beginPath(),s.moveTo(O,H),s.lineTo(q,K),s.stroke(),s.shadowColor="hsl(42 95% 62%)",s.shadowBlur=10,s.fillStyle="hsl(42 95% 74%)",s.beginPath(),s.arc(q,K,2.8,0,Math.PI*2),s.fill(),s.restore()}function Ie(C,L,O){let H=tn;J(C,L,.22,H,26,"hsl(220 10% 34%)","hsl(220 10% 16%)","hsl(220 10% 24%)");let q=L+.22;for(let K=0;K<5;K++)for(let re=0;re<2;re++){let[pe,me]=_(C-.1+re*.12,q,H+4+K*4.5),B=Math.sin(O/170+K*1.7+re*2.9)>.1;s.beginPath(),s.arc(pe,me,1.3,0,Math.PI*2),s.fillStyle=B?re?g.ok:g.info:g.ink3,s.fill()}}function We(C,L){let O=tn;J(C,L,.3,O,24,"hsl(28 30% 62%)","hsl(28 28% 40%)","hsl(28 28% 48%)");let H=L+.3;for(let q=0;q<2;q++){let K=O+4+q*10;for(let re=0;re<6;re++){let pe=C-.26+re*.09,me=[_(pe,H,K+7),_(pe+.07,H,K+7),_(pe+.07,H,K),_(pe,H,K)];k(me,`hsl(${(re*53+q*140)%360} 45% ${q?46:58}%)`)}}}function He(C,L,O){let H=tn;s.save(),s.strokeStyle=g.ink2,s.lineWidth=1.2;for(let K of[C-.5,C+.5]){let[re,pe]=_(K,L,H),[me,B]=_(K,L,H+28);s.beginPath(),s.moveTo(re,pe),s.lineTo(me,B),s.stroke()}let q=[_(C-.55,L,H+26),_(C+.55,L,H+26),_(C+.55,L,H+12),_(C-.55,L,H+12)];k(q,g.sheet),s.strokeStyle=g.ink3,s.lineWidth=1,s.beginPath(),q.forEach(([K,re],pe)=>pe===0?s.moveTo(K,re):s.lineTo(K,re)),s.closePath(),s.stroke(),s.strokeStyle=O?g.info:g.signal,s.beginPath();for(let K=0;K<3;K++){let re=H+22-K*3.5,[pe,me]=_(C-.4,L,re),[B,ue]=_(C-.4+.25+K*.12,L,re);s.moveTo(pe,me),s.lineTo(B,ue)}s.stroke(),s.restore()}function oe(C,L){J(C,L,.3,0,6,"hsl(30 22% 66%)","hsl(30 20% 44%)","hsl(30 20% 52%)")}function le(C,L){let O=a.some(H=>H.district===C.d&&H.agent.status==="working");switch(C.kind){case"desk":ie(C.x,C.y,O,L);break;case"plant":ce(C.x,C.y);break;case"crate":Se(C.x,C.y);break;case"lamp":fe(C.x,C.y);break;case"rack":Ie(C.x,C.y,L);break;case"shelf":We(C.x,C.y);break;case"board":He(C.x,C.y,O);break;case"bench":oe(C.x,C.y);break}}function be(C){let L=o.find(re=>re.role==="orchestrator");if(!L)return;let[O,H]=_(L.cx,L.cy,tn+60+Math.sin(C/900)*2),q=22*u,K=C/2600;s.save(),s.globalAlpha=.7,s.strokeStyle=g.signal,s.lineWidth=1.4,s.setLineDash([6*u,5*u]),s.lineDashOffset=-C/60,s.beginPath(),s.ellipse(O,H,q,q*.36,K,0,Math.PI*2),s.stroke(),s.globalAlpha=.4,s.strokeStyle=g.info,s.setLineDash([]),s.beginPath(),s.ellipse(O,H,q*.66,q*.24,-K*1.6,0,Math.PI*2),s.stroke(),s.restore()}function Ve(C,L){let[O,H]=_(C.x,C.y,tn),q=C.agent.status,K=q==="working"?g.info:q==="idle"?g.ok:g.ink3,re=C.district.hue,pe=C.moving?Math.sin(C.walk)*3:0,me=ue=>q==="working"?Math.sin(L/110+(ue>0?Math.PI:0))*2:0;s.save(),s.globalAlpha=q==="offline"?.4:1,s.translate(O,H),s.scale(u*C.facing,u),s.save(),s.globalAlpha*=.18,s.fillStyle=g.ink,s.beginPath(),s.ellipse(0,0,9,4.5,0,0,Math.PI*2),s.fill(),s.restore(),s.strokeStyle=g.ink2,s.lineWidth=2.5,s.lineCap="round";for(let ue of[-1,1]){let se=ue===-1?pe:-pe;s.beginPath(),s.moveTo(ue*3.5,-12),s.lineTo(ue*3.5+se,0),s.stroke()}let B=s.createLinearGradient(0,-30,0,-12);B.addColorStop(0,`hsl(${re} 55% 66%)`),B.addColorStop(1,`hsl(${re} 50% 44%)`),N(-7,-30,14,18,4),s.fillStyle=B,s.fill(),s.strokeStyle=g.ink2,s.lineWidth=.8,s.stroke(),N(-4,-25,8,7,1.5),s.fillStyle=g.sheet,s.fill(),s.fillStyle=g.ink,s.font="bold 7px ui-monospace, SFMono-Regular, Menlo, monospace",s.textAlign="center",s.textBaseline="middle",s.fillText(C.agent.role.charAt(0).toUpperCase()||"?",0,-21.5),s.strokeStyle=`hsl(${re} 45% 36%)`,s.lineWidth=2;for(let ue of[-1,1])s.beginPath(),s.moveTo(ue*7,-27),s.lineTo(ue*10,-19+me(ue)),s.stroke();N(-6,-41,12,10,3),s.fillStyle=g.sheet,s.fill(),s.strokeStyle=g.ink2,s.stroke(),N(-4.5,-38.5,9,5,1.5),s.fillStyle=g.ink,s.fill(),s.fillStyle=K;for(let ue of[-2,2])s.beginPath(),s.arc(ue,-36,1.1,0,Math.PI*2),s.fill();if(s.strokeStyle=g.ink2,s.lineWidth=.8,s.beginPath(),s.moveTo(0,-41),s.lineTo(0,-46),s.stroke(),s.beginPath(),s.arc(0,-47,1.8,0,Math.PI*2),s.fillStyle=K,s.fill(),q==="working"){let ue=.5+.5*Math.sin(L/300);s.shadowColor=g.info,s.shadowBlur=8+6*ue,s.beginPath(),s.arc(0,-56,2.6+ue*.6,0,Math.PI*2),s.fillStyle=g.info,s.fill(),s.shadowBlur=0}s.restore()}function Me(C,L){let[O,H]=_(C.x,C.y,tn);if(C.bubble&&L<C.bubble.until){let[q,K]=_(C.x,C.y,tn+52),re=Cr((C.bubble.until-L)/800,0,1),pe=Qt(C.bubble.text,30);s.font="11px ui-sans-serif, system-ui, sans-serif";let me=s.measureText(pe).width+14,B=18;s.save(),s.globalAlpha=re,N(q-me/2,K-B-6,me,B,6),s.fillStyle=g.sheet,s.fill(),s.strokeStyle=C.bubble.tone==="danger"?g.danger:C.bubble.tone==="ok"?g.ok:g.info,s.lineWidth=1,s.stroke(),s.fillStyle=g.ink,s.textAlign="center",s.textBaseline="middle",s.fillText(pe,q,K-B/2-6),s.restore()}(C===m||C===f||u>.9)&&(s.font="600 11px ui-sans-serif, system-ui, sans-serif",s.textAlign="center",s.textBaseline="top",s.lineJoin="round",s.lineWidth=3,s.strokeStyle=g.paper,s.strokeText(C.agent.name,O,H+14),s.fillStyle=g.ink,s.fillText(C.agent.name,O,H+14))}function Ge(C){for(let L=h.length-1;L>=0;L--){let O=h[L];if(!O)continue;let H=(C-O.start)/q1;if(H>=1){h.splice(L,1);continue}let q=c.get(O.from),K=c.get(O.to);if(!q||!K)continue;let[re,pe]=_(q.x,q.y,tn+_f),[me,B]=_(K.x,K.y,tn+_f),ue=(re+me)/2,se=(pe+B)/2-60*u,ge=1-H,ye=ge*ge*re+2*ge*H*ue+H*H*me,de=ge*ge*pe+2*ge*H*se+H*H*B,Pe=O.tone==="ok"?g.ok:g.signal;s.save(),s.shadowColor=Pe,s.shadowBlur=12,s.fillStyle=Pe,s.beginPath(),s.arc(ye,de,3.5,0,Math.PI*2),s.fill(),s.restore()}}function Bt(C){s.setTransform(v,0,0,v,0,0),s.globalAlpha=1,s.fillStyle=g.paper,s.fillRect(0,0,p,x);let L=8;for(let H of o)L=Math.max(L,Math.abs(H.cx)+H.r+3,Math.abs(H.cy)+H.r+3);s.save(),s.globalAlpha=.3,s.strokeStyle=g.rule,s.lineWidth=1,s.beginPath();for(let H=-L;H<=L;H++){let[q,K]=_(H,-L),[re,pe]=_(H,L);s.moveTo(q,K),s.lineTo(re,pe);let[me,B]=_(-L,H),[ue,se]=_(L,H);s.moveTo(me,B),s.lineTo(ue,se)}s.stroke(),s.restore(),Y();for(let H of o)U(H);for(let H of l)H.kind==="lamp"&&ee(H.x,H.y);let O=[];for(let H of l)O.push({depth:H.x+H.y,paint:()=>le(H,C)});for(let H of a){let q=H.agent.status==="working",[K,re]=H.home;O.push({depth:K+re-.01,paint:()=>z(K,re,q)}),O.push({depth:H.x+H.y,paint:()=>Ve(H,C)})}O.sort((H,q)=>H.depth-q.depth);for(let H of O)H.paint();be(C),Ge(C);for(let H of a)Me(H,C);nn()}function Je(C,L,O){C!==L&&(h.push({from:C,to:L,start:performance.now(),tone:O}),h.length>Y1&&h.shift())}function st(C,L,O){C.bubble={text:L,until:performance.now()+X1,tone:O}}function mt(C,L){let O=null,H=22*u;for(let q of a){let[K,re]=_(q.x,q.y,tn),pe=Math.hypot(C-K,L-(re-24*u));pe<H&&(H=pe,O=q)}return O}function tt(C){f=C,b="",W()}let Et=C=>C.agent.status==="offline"?"offline":C.activity,Ht=C=>{let L=typeof C=="number"?c.get(C):void 0;return L?L.agent.name:String(C??"?")};function $t(C,L){C.activity=L,C===f&&W()}function It(C,L){let O=L.to_cid??L.assignee;return(C==="plaza_send_message"||(C==="plaza_assign"||C==="plaza_post_task"))&&typeof O=="number"?O:null}function zt(C,L){if(C==="bash")return`$ ${Qt(Fi(L.command).split(`
`)[0]??"",120)}`;if(C==="plaza_send_message")return`\u2709 to ${Ht(L.to_cid)}: ${Qt(Fi(L.text),100)}`;if(C==="plaza_assign"||C==="plaza_post_task"&&L.assignee!=null)return`\u2192 task for ${Ht(L.assignee??L.to_cid)}: ${Fi(L.title)}`;if(C==="share_file")return`\u21EA shared ${Fi(L.path)}`;let O=Fi(L.path)||Fi(L.url);return O?`\u2192 ${C} ${Qt(O,80)}`:`\u2192 ${C}`}function W(){if(!f){t.hidden=!0,b="";return}let C=f.agent;t.hidden=!1;let L=Et(f),O=[C.name,L].join("|");O!==b&&(b=O,t.replaceChildren(y("header.floor-card-head",null,y("strong",null,C.name)),y("div.floor-card-activity",null,L)))}function nn(){if(!f||t.hidden)return;let[C,L]=_(f.x,f.y,tn+40),O=Cr(C+24,8,Math.max(8,p-308)),H=Cr(L-120,8,Math.max(8,x-260));t.style.left=`${O}px`,t.style.top=`${H}px`}let ut=C=>{let L=e.getBoundingClientRect();return[C.clientX-L.left,C.clientY-L.top]};e.addEventListener("pointerdown",C=>{P={x:C.clientX,y:C.clientY,moved:0},e.setPointerCapture(C.pointerId),e.style.cursor="grabbing"}),e.addEventListener("pointermove",C=>{if(P){let H=C.clientX-P.x,q=C.clientY-P.y;P.moved+=Math.abs(H)+Math.abs(q),P.x=C.clientX,P.y=C.clientY,d={x:d.x+H,y:d.y+q};return}let[L,O]=ut(C);m=mt(L,O),e.style.cursor=m?"pointer":"grab"}),e.addEventListener("pointerup",C=>{if(!P)return;let L=P.moved;if(P=null,e.style.cursor="grab",L<4){let[O,H]=ut(C);tt(mt(O,H))}}),e.addEventListener("pointercancel",()=>{P=null,e.style.cursor="grab"}),e.addEventListener("pointerleave",()=>{m=null}),e.addEventListener("wheel",C=>{C.preventDefault();let[L,O]=ut(C),H=p/2+d.x,q=x/2+d.y,K=(L-H)/u,re=(O-q)/u;u=Cr(u*Math.exp(-C.deltaY*.0015),.4,2.5),d={x:L-K*u-p/2,y:O-re*u-x/2}},{passive:!1});let F=()=>{v=window.devicePixelRatio||1,p=e.clientWidth,x=e.clientHeight,e.width=Math.round(p*v),e.height=Math.round(x*v)},M=new ResizeObserver(()=>F());M.observe(e),F();let Z=dt.watch(C=>{S(C),!w&&C.length&&(A(),w=!0)}),te=Mt("transcript",C=>{let L=C,O=c.get(L.cid);switch(L.kind){case"tool_request":{let H=Fi(L.data.tool)||Fi(L.data.name)||"tool",q=L.data.input??{};O&&(st(O,`\u2192 ${H}`,"info"),$t(O,zt(H,q)));let K=It(H,q);K!==null&&Je(L.cid,K,"accent");break}case"model_request":O&&$t(O,"thinking\u2026");break;case"work_end":{let H=Fi(L.data.status)||"done";O&&(st(O,H,/err|fail/i.test(H)?"danger":"ok"),$t(O,"idle"));break}case"work_start":{let H=L.data.origin??{};if(typeof H.from_cid=="number")Je(H.from_cid,L.cid,"accent");else if(L.data.kind==="task"){let q=a.find(K=>K.agent.tier==="orchestrator");q&&Je(q.agent.cid,L.cid,"accent")}break}default:break}}),ae=Mt("delta",C=>{let L=c.get(C.cid);L&&$t(L,"thinking\u2026")});e.addEventListener("dblclick",C=>{let[L,O]=ut(C),H=mt(L,O);H&&Mn("agents",H.agent.cid)});let ve=Mt("conversation",C=>{let L=C;Je(L.from_cid,L.to_cid,"ok")}),_e=C=>{if(!T)return;let L=Math.min(.05,(C-D)/1e3);D=C,I(L,C),Bt(C),E=requestAnimationFrame(_e)};return E=requestAnimationFrame(_e),()=>{T=!1,cancelAnimationFrame(E),M.disconnect(),Z(),te(),ae(),ve(),n.classList.remove("floor-view"),n.replaceChildren()}}var sy=1500,oy=1.6,ay=60,kr=.5,Ul=.8,Eo=2.3,Ss=1.5,wu=2.2,ly=9.8,cy=.95,uy=Math.PI/4,hy=9.5,wf=3.1,To=.55,dy=[0,30,-30,60,-60].map(n=>n*Math.PI/180),Eu=90,fy="position:absolute;left:0;top:0;display:none;max-width:220px;padding:3px 6px;background:var(--sheet);border:1px solid var(--ink);color:var(--ink);font:11px/1.35 ui-monospace,SFMono-Regular,Menlo,monospace;white-space:nowrap;pointer-events:none;will-change:transform",Ef=[8,28,46,88,140,172,198,222,262,340];function Au(n){return(n*137.508+20)%360}function Ru(n){let e=0;for(let t of n)e=e*31+t.charCodeAt(0)>>>0;return Ef[e%Ef.length]}function py(n){let e=new Map;for(let p of n){let x=p.tier==="orchestrator"?"":p.role;e.set(x,[...e.get(x)??[],p])}let t=[],i=new Map,r=(p,x)=>{t.push(p);let v=Math.max(1,Math.ceil(Math.sqrt(x.length)));x.sort((m,f)=>m.cid-f.cid).forEach((m,f)=>{let b=1.5*(v-1),P=p.cx-b+f%v*3,w=p.cy-b+Math.floor(f/v)*3;i.set(m.cid,[p,P,w])})},s=e.get("");s&&r({role:"orchestrator",cx:0,cy:0,r:2,hue:25,shape:"round"},s);let o=[...e.keys()].filter(p=>p).sort(),a=p=>{let x=e.get(p).length;return Math.max(wo(p).size,1.5*Math.max(1,Math.ceil(Math.sqrt(x)))+1.5)},l=2.4,c=o.map(p=>2*(a(p)+Ss)+l),h=c.reduce((p,x)=>p+x,0),d=Math.max(10+Ss,h/(2*Math.PI)),u=0;return o.forEach((p,x)=>{let v=e.get(p),m=(u+c[x]/2)/h*Math.PI*2-Math.PI/2;u+=c[x];let f=a(p),b=wo(p).shape==="square"?d+f*.3:d;r({role:p,cx:Math.cos(m)*b,cy:Math.sin(m)*b,r:f,hue:Ru(p),shape:wo(p).shape},v)}),{districts:t,homes:i}}function my(){let n=getComputedStyle(document.documentElement),e=(t,i)=>n.getPropertyValue(`--${t}`).trim()||i;return{paper:e("paper","#f5f3ee"),sheet:e("sheet","#ffffff"),rule:e("rule","#d6d2c8"),ink:e("ink","#1a1a1a"),ink2:e("ink-2","#4a4a4a"),ink3:e("ink-3","#8a8a8a"),signal:e("signal","#d9482b"),ok:e("ok","#2f8f5b"),info:e("info","#2f6fb0"),danger:e("danger","#c0392b")}}var Pr=null;function Nl(n,e){Pr??=document.createElement("canvas").getContext("2d");let t=new ke;if(!Pr)return t.set(e);Pr.fillStyle=e,Pr.fillStyle=n,Pr.clearRect(0,0,1,1),Pr.fillRect(0,0,1,1);let i=Pr.getImageData(0,0,1,1).data;return t.setRGB((i[0]??0)/255,(i[1]??0)/255,(i[2]??0)/255,Pt)}function ar(n){return typeof n=="string"?n:""}function Tu(n,e,t){return Math.min(t,Math.max(e,n))}var Ms=n=>n.role==="orchestrator"?Ul:kr,Tf=(n,e,t)=>e>n.x0-To&&e<n.x1+To&&t>n.z0-To&&t<n.z1+To;function Ir(n){n.traverse(e=>{let t=e;t.geometry?.dispose();let i=Array.isArray(t.material)?t.material:t.material?[t.material]:[];for(let r of i)r.map?.dispose(),r.dispose()})}function gy(n,e){let t=e.to_cid??e.assignee;return(n==="plaza_send_message"||(n==="plaza_assign"||n==="plaza_post_task"))&&typeof t=="number"?t:null}function xy(n,e,t){if(n==="bash")return`$ ${Qt(ar(e.command).split(`
`)[0]??"",120)}`;if(n==="plaza_send_message")return`\u2709 to ${t(e.to_cid)}: ${Qt(ar(e.text),100)}`;if(n==="plaza_assign"||n==="plaza_post_task"&&e.assignee!=null)return`\u2192 task for ${t(e.assignee??e.to_cid)}: ${ar(e.title)}`;if(n==="share_file")return`\u21EA shared ${ar(e.path)}`;let i=ar(e.path)||ar(e.url);return i?`\u2192 ${n} ${Qt(i,80)}`:`\u2192 ${n}`}function vy(){try{return new ys({antialias:!0})}catch{return null}}function Af(n){let e=document.createElement("canvas"),i=e.getContext("webgl2")??e.getContext("webgl")?vy():null;if(!i)return Sf(n);let r=my(),s={sheet:new ke(16774366),rule:new ke(14858623),ink3:new ke(6715538),signal:Nl(r.signal,"#d9482b"),ok:Nl(r.ok,"#2f8f5b"),info:Nl(r.info,"#2f6fb0"),paper:Nl(r.paper,"#f5f3ee")},o={ink:r.ink,sheet:r.sheet,paper:r.paper,rule:r.rule,signal:r.signal,ok:r.ok,info:r.info,inkMuted:r.ink3};i.setPixelRatio(Math.min(window.devicePixelRatio||1,2)),i.shadowMap.enabled=!0,i.shadowMap.type=Mr;let a=i.domElement;a.classList.add("floor-canvas"),a.setAttribute("aria-label","The plaza floor"),a.style.cssText="position:absolute;inset:0;width:100%;height:100%;display:block;cursor:grab";let l=new xr,c=new ke(6269160),h=new ke(16640452);l.add(Yd(c,h,new ke(9424226))),l.fog=new Gs(h,70,240);let d=Zd(16,120,Ui(31));l.add(d.group);let u=new cn(40,1,.5,1e3),p=new Cl(u,a);p.enableDamping=!0,p.dampingFactor=.08,p.enablePan=!0,p.enableZoom=!0,p.minPolarAngle=.2,p.maxPolarAngle=1.3,p.minDistance=6,p.maxDistance=300,l.add(new _r(14676223,9075290,1.1));let x=new br(16773590,2.6);x.position.set(22,40,16),x.castShadow=!0,x.shadow.mapSize.set(2048,2048),x.shadow.bias=-8e-4,x.shadow.normalBias=.03;let v=x.shadow.camera;Object.assign(v,{left:-40,right:40,top:40,bottom:-40,near:1,far:140}),v.updateProjectionMatrix(),l.add(x);let m=new $e(new Xn(500,500),it({map:bu()}));bu().repeat.set(36,36),m.rotation.x=-Math.PI/2,m.position.y=-.01,m.receiveShadow=!0,l.add(m);let f=new Oe;l.add(f);let b=new Oe;l.add(b);let P=of(s);l.add(P);let w=P.userData.animate,T=new In(.42,28),E=new Lt(.13,14,10),D=new Lt(.34,14,10),g=new bt(.2,.2,.2),S={accent:new Ot({color:s.signal,toneMapped:!1}),ok:new Ot({color:s.ok,toneMapped:!1}),accentHalo:new Ot({color:s.signal,transparent:!0,opacity:.35,depthWrite:!1,blending:Li,toneMapped:!1}),okHalo:new Ot({color:s.ok,transparent:!0,opacity:.35,depthWrite:!1,blending:Li,toneMapped:!1}),token:new Ot({color:s.signal,toneMapped:!1})},_=new Float32Array(Eu*3),A=new Cn(_,3),I=[];for(let X=0;X<Eu;X++){let he=Math.random()*Math.PI*2,xe=Math.sqrt(Math.random())*9;I.push({x:Math.cos(he)*xe,y:Math.random()*4.2,z:Math.sin(he)*xe,rise:.04+Math.random()*.05})}let k=new Yt;k.setAttribute("position",A);let N=new as({color:s.ink3,size:.07,transparent:!0,opacity:.55,depthWrite:!1});l.add(new $s(k,N));let U=y("div.floor-card",{hidden:!0}),z=y("div.floor-legend",null,y("span",null,y("i.floor-key.working"),"Working"),y("span",null,y("i.floor-key.idle"),"Idle"),y("span",null,y("i.floor-key.offline"),"Offline")),Y=document.createElement("div");Y.style.cssText="position:absolute;inset:0;overflow:hidden;pointer-events:none";let ee=y("div.floor-stage",null,Y,U,z);ee.style.position="relative",ee.prepend(a),n.classList.add("floor-view"),document.body.classList.add("floor-mode"),n.append(y("div.view-head",null,y("h1",null,"Floor"),y("div.actions",null,Ze("Reset view",()=>Z(),{kind:"ghost",icon:"refresh"}))),ee);let J=[],ie="",ce=[],Se=new Map,fe=[],Ie=[],We=new Map,He=[],oe=new G,le=0,be=0,Ve=null,Me=null,Ge="",Bt=!1,Je=performance.now(),st=new Xe,mt=new io,tt=new G,Et=new G(0,1,0),Ht=new G,$t=new un,It=new un;function zt(X,he){let xe=document.createElement("canvas");xe.width=512,xe.height=128;let $=xe.getContext("2d");$.beginPath(),$.moveTo(24,4),$.arcTo(508,4,508,124,20),$.arcTo(508,124,4,124,20),$.arcTo(4,124,4,4,20),$.arcTo(4,4,508,4,20),$.closePath(),$.fillStyle=r.sheet,$.fill(),$.lineWidth=6,$.strokeStyle=he?r.signal:r.rule,$.stroke(),$.fillStyle=r.ink2,$.textAlign="center",$.textBaseline="middle";let Re="500 48px ui-monospace, SFMono-Regular, Menlo, monospace";$.font=Re;let we=$.measureText(X).width;we>460&&($.font=`500 ${Math.floor(48*460/we)}px ui-monospace, SFMono-Regular, Menlo, monospace`),$.fillText(X,256,66);let Qe=new Ii(xe);return Qe.colorSpace=Pt,Qe}function W(X){let he=new Oe,xe=X.role==="orchestrator";if(xe){let Re=new $e(new Wt(Eo,Eo+wu,Ul,48),it({map:bo(new ke(15782818),3)}));Re.position.set(X.cx,Ul/2,X.cy),Re.castShadow=!0,Re.receiveShadow=!0;let we=new $e(new Un(Eo,.05,8,64),new Ot({color:s.signal}));we.rotation.x=Math.PI/2,we.position.set(X.cx,Ul+.01,X.cy),he.add(Re,we)}else{let Re=X.shape==="round",we=Re?new Wt(X.r,X.r+Ss,kr,48,1):new Wt(X.r*Math.SQRT2,(X.r+Ss)*Math.SQRT2,kr,4,1).rotateY(Math.PI/4),Qe=new $e(we,nf(X.hue));Qe.position.set(X.cx,kr/2,X.cy),Qe.castShadow=!0,Qe.receiveShadow=!0,he.add(Qe);let qe=Math.atan2(-X.cx,-X.cy),Gt=Re?qe:Math.round(qe/(Math.PI/2))*(Math.PI/2),ct=yf(X.role,X.r,X.hue,s),Kt=new $e(Re?new In(X.r,48):new Xn(2*X.r,2*X.r),ct.floor);Kt.rotation.set(-Math.PI/2,0,Gt),Kt.position.set(X.cx,kr+.004,X.cy),Kt.receiveShadow=!0,he.add(Kt),ct.group.position.set(X.cx,kr,X.cy),ct.group.rotation.y=Gt,he.add(ct.group),ct.group.updateMatrixWorld(!0);for(let Yn of ct.group.children){if(Yn.userData.walk)continue;let kn=new Nn().setFromObject(Yn);kn.isEmpty()||kn.min.y>kr+2.2||Ie.push({x0:kn.min.x,x1:kn.max.x,z0:kn.min.z,z1:kn.max.z})}We.set(X.role,{screens:ct.screens,lit:!1}),He.push(ct.animate)}let $=new $e(new Xn(2.6,.65),new Ot({map:zt(X.role.toUpperCase(),xe),transparent:!0,depthWrite:!1}));if(xe)$.rotation.x=-Math.PI/2,$.position.set(X.cx,Ms(X)+.01,X.cy-1.6),he.add($);else{let Re=new Oe;Re.position.set(X.cx,Ms(X)+.012,X.cy),Re.rotation.y=Math.atan2(-X.cx,-X.cy),$.rotation.x=-Math.PI/2,$.position.set(0,0,X.r-.5),Re.add($),he.add(Re)}return ei(he),he}function nn(X){for(let we of[...f.children])f.remove(we),Ir(we);We.clear(),He.length=0,Ie.length=0;for(let we of X)f.add(W(we));Ir(b),b.clear(),b.add(lf(X.map(we=>new G(we.cx,0,we.cy)),s));let he=af(s,X.map(we=>Math.atan2(we.cy,we.cx)));b.add(he.group),He.push(he.animate);for(let we of he.boxes)Ie.push(we);let xe=cf(X,s);b.add(xe.group);for(let we of xe.boxes)Ie.push(we);let $=12;for(let we of X)$=Math.max($,Math.hypot(we.cx,we.cy)+we.r+10);let Re=Kd(9e3,Math.min($,60),(we,Qe)=>{if(Math.hypot(we,Qe)<4.8)return!1;for(let qe of X){let Gt=qe.role==="orchestrator"?Eo+wu:qe.r+Ss;if(Math.abs(we-qe.cx)<Gt+.2&&Math.abs(Qe-qe.cy)<Gt+.2)return!1;let ct=Math.hypot(qe.cx,qe.cy);if(ct>.5){let Kt=Tu((we*qe.cx+Qe*qe.cy)/(ct*ct),0,1);if(Math.hypot(we-qe.cx*Kt,Qe-qe.cy*Kt)<.75)return!1}}return!Ie.some(qe=>we>qe.x0-.1&&we<qe.x1+.1&&Qe>qe.z0-.1&&Qe<qe.z1+.1)},Ui(97));b.add(Re.mesh),He.push(Re.animate)}function ut(X,he,xe,$){let Re=X.tier==="orchestrator",we=Il(he.hue,Re,o,Au(X.cid));we.group.traverse(kn=>{kn.castShadow=!0,kn.receiveShadow=!0}),l.add(we.group);let Qe=new Ot({color:s.ink3,transparent:!0,opacity:.6,depthWrite:!1}),qe=new $e(T,Qe);qe.rotation.x=-Math.PI/2,qe.position.set(xe,Ms(he)+.02,$),l.add(qe);let Gt=new $e(g,S.token);Gt.visible=!1,l.add(Gt);let ct=document.createElement("div");ct.style.cssText=fy;let Kt=document.createElement("strong");Kt.style.cssText="display:block;overflow:hidden;text-overflow:ellipsis";let Yn=document.createElement("div");return Yn.style.cssText="color:var(--ink-2);overflow:hidden;text-overflow:ellipsis",ct.append(Kt,Yn),Y.append(ct),{agent:X,district:he,home:[xe,$],x:xe,y:$,tx:xe,ty:$,heading:0,moving:!1,py:0,vy:0,state:null,nextWander:performance.now()+Math.random()*3e3,nextSocial:performance.now()+4e3+Math.random()*1e4,social:null,activity:"idle",robot:we,robotKey:`${he.hue}|${Re}`,tilt:new un,pad:qe,padMat:Qe,token:Gt,highlighted:!1,label:ct,labelName:Kt,labelLine:Yn}}function F(X){l.remove(X.robot.group),X.robot.dispose(),l.remove(X.pad),X.padMat.dispose(),l.remove(X.token),X.label.remove()}function M(X){let he=py(X);J=he.districts;let xe=JSON.stringify(J);xe!==ie&&(ie=xe,nn(J));let $=new Set;for(let Re of X){let we=he.homes.get(Re.cid);if(!we)continue;let[Qe,qe,Gt]=we;$.add(Re.cid);let ct=Se.get(Re.cid);ct&&ct.robotKey!==`${Qe.hue}|${Re.tier==="orchestrator"}`&&(F(ct),ct=void 0),ct||(ct=ut(Re,Qe,qe,Gt),Se.set(Re.cid,ct)),Object.assign(ct,{agent:Re,district:Qe,home:[qe,Gt]}),ct.pad.position.set(qe,Ms(Qe)+.02,Gt)}for(let[Re,we]of Se)$.has(Re)||(F(we),Se.delete(Re));ce=[...Se.values()],Me&&(Me=Se.get(Me.agent.cid)??null),Ve&&(Ve=Se.get(Ve.agent.cid)??null),se()}function Z(){let X=6;for(let xe of J)X=Math.max(X,Math.abs(xe.cx)+xe.r,Math.abs(xe.cy)+xe.r);let he=X*2.6+6;p.target.set(0,.4,0),u.position.copy(p.target).add(new G().setFromSphericalCoords(he,cy,uy)),p.update()}let te=(X,he)=>X.role===he.role||Math.hypot(X.cx-he.cx,X.cy-he.cy)<=hy;function ae(X,he){if(X.nextSocial=he+8e3+Math.random()*12e3,Math.random()<.3){let Re=Math.atan2(X.y,X.x);X.social={phase:"stroll",partner:null,gx:Math.cos(Re)*wf,gy:Math.sin(Re)*wf,until:1/0};return}let xe=ce.filter(Re=>Re!==X&&Re.agent.status==="idle"&&!Re.social&&te(Re.district,X.district)),$=xe[Math.floor(Math.random()*xe.length)];$&&(X.social={phase:"go",partner:$,gx:$.x-1,gy:$.y,until:1/0},$.social={phase:"talk",partner:X,gx:$.x,gy:$.y,until:1/0})}function ve(X,he){let xe=X.social;X.social=null,X.nextWander=he,xe?.partner&&xe.partner.social?.partner===X&&(xe.partner.social=null,xe.partner.nextWander=he)}function _e(X,he){let xe=0;for(let $ of J){let Re=$.role==="orchestrator",we=Re?Math.hypot(X-$.cx,he-$.cy)-Eo:$.shape==="round"?Math.hypot(X-$.cx,he-$.cy)-$.r:Math.max(Math.abs(X-$.cx),Math.abs(he-$.cy))-$.r,Qe=Re?wu:Ss,qe=we<=0?Ms($):we<Qe?Ms($)*(1-we/Qe):0;xe=Math.max(xe,qe)}return xe}function C(X,he,xe,$){if($){for(let we of Ie)if(Tf(we,he,xe)&&!Tf(we,X.x,X.y))return!0}let Re=X.social?.partner??null;for(let we of ce){if(we===X||we===Re)continue;let Qe=Math.hypot(we.x-he,we.y-xe);if(Qe<To*2&&Qe<Math.hypot(we.x-X.x,we.y-X.y))return!0}return!1}function L(X,he){let xe=he/1e3;for(let $ of We.values())$.lit=!1;for(let $ of ce){let Re=$.agent.status,we=Re==="idle",Qe=$.social?.partner??null;if($.social&&(!we||Qe&&(Se.get(Qe.agent.cid)!==Qe||Qe.agent.status!=="idle"))&&ve($,he),we&&!$.social){if(he>=$.nextWander){let Ae=Math.max(.5,$.district.r-.7);$.tx=$.district.cx+(Math.random()*2-1)*Ae,$.ty=$.district.cy+(Math.random()*2-1)*Ae,$.nextWander=he+3e3+Math.random()*4e3}he>=$.nextSocial&&ae($,he)}let qe=$.social,Gt=$.home[0],ct=$.home[1];we&&(Gt=qe?qe.gx:$.tx,ct=qe?qe.gy:$.ty);let Kt=Gt-$.x,Yn=ct-$.y,kn=Math.hypot(Kt,Yn);$.moving=kn>.02;let Xl=Math.hypot($.home[0]-$.x,$.home[1]-$.y)>.9,Co=null;if($.moving){let Ae=Math.min(oy*X,kn),Ue=Math.atan2(Kt,Yn);for(let et of dy){let nt=Ue+et,Fe=$.x+Math.sin(nt)*Ae,gt=$.y+Math.cos(nt)*Ae;if(!C($,Fe,gt,Xl)){$.x=Fe,$.y=gt,Co=nt;break}}}if(qe&&we){if(qe.phase==="go"&&!$.moving){let Ae=he+3e3+Math.random()*2e3;qe.phase="talk",qe.until=Ae,qe.partner?.social&&(qe.partner.social.until=Ae)}else qe.phase==="stroll"&&!$.moving&&qe.until===1/0&&(qe.until=he+2e3);he>=qe.until&&ve($,he)}let Lr=qe&&qe.phase==="talk"?qe.partner:null,R=null;if(Lr?R=Math.atan2(Lr.x-$.x,Lr.y-$.y):$.moving&&(R=Co??Math.atan2(Kt,Yn)),R!==null){let Ae=Math.atan2(Math.sin(R-$.heading),Math.cos(R-$.heading));$.heading+=Ae*Math.min(1,X*8)}let V=Re==="working"?"working":Re==="offline"?"offline":$.moving?"walking":"idle";V!==$.state&&($.state=V,$.robot.setState(V));let ne=$===Me||$===Ve;ne!==$.highlighted&&($.highlighted=ne,$.robot.setHighlighted(ne));let Q=_e($.x,$.y);$.py>Q+.001?($.vy-=ly*X,$.py=Math.max(Q,$.py+$.vy*X),$.py===Q&&($.vy=0)):($.py=Q,$.vy=0),$.robot.group.position.set($.x,$.py,$.y);let j=.3,Ce=(_e($.x+j,$.y)-_e($.x-j,$.y))/(2*j),De=(_e($.x,$.y+j)-_e($.x,$.y-j))/(2*j);Ht.set(-Ce,1,-De).normalize(),$t.setFromUnitVectors(Et,Ht).multiply(It.setFromAxisAngle(Et,$.heading)),$.tilt.slerp($t,1-Math.exp(-10*X)),$.robot.group.quaternion.copy($.tilt),$.robot.update(X,xe),$.padMat.color.copy(Re==="working"?s.info:Re==="idle"?s.ok:s.ink3);let Ee=Re==="working";if($.token.visible=Ee,Ee){let Ae=We.get($.district.role);Ae&&(Ae.lit=!0);let Ue=$.robot.labelAnchor(oe);$.token.position.set(Ue.x,Ue.y+.6+Math.sin(xe*2.5+$.agent.cid)*.08,Ue.z),$.token.rotation.y+=X*1.8}}for(let $ of We.values()){let Re=$.lit?1.2:.05;for(let we of $.screens)we.emissiveIntensity!==Re&&(we.emissiveIntensity=Re)}}function O(X){for(let he=0;he<Eu;he++){let xe=I[he],$=he*3;_[$]=xe.x+Math.sin(X*.3+he)*.25,_[$+1]=.3+(xe.y+X*xe.rise)%4.2,_[$+2]=xe.z+Math.cos(X*.25+he)*.25}A.needsUpdate=!0}function H(X,he,xe){if(X===he)return;let $=new $e(E,xe==="ok"?S.ok:S.accent),Re=new $e(D,xe==="ok"?S.okHalo:S.accentHalo);for(l.add($,Re),fe.push({from:X,to:he,start:performance.now(),core:$,halo:Re});fe.length>ay;)q(fe.shift())}function q(X){l.remove(X.core,X.halo)}function K(X){for(let he=fe.length-1;he>=0;he--){let xe=fe[he],$=(X-xe.start)/sy,Re=Se.get(xe.from),we=Se.get(xe.to);if($>=1||!Re||!we){q(xe),fe.splice(he,1);continue}let Qe=Re.robot.labelAnchor(new G),qe=we.robot.labelAnchor(new G),Gt=Qe.clone().lerp(qe,.5);Gt.y+=.8+Qe.distanceTo(qe)*.3;let ct=1-$,Kt=Qe.multiplyScalar(ct*ct).addScaledVector(Gt,2*ct*$).addScaledVector(qe,$*$);xe.core.position.copy(Kt),xe.halo.position.copy(Kt)}}let re=X=>X.agent.status==="offline"?"offline":X.social?.phase==="talk"?"\xB7 \xB7 \xB7":X.activity,pe=X=>{let he=typeof X=="number"?Se.get(X):void 0;return he?he.agent.name:String(X??"?")};function me(X,he){X.activity=he,X===Me&&se()}function B(X){let he=X.clone().project(u);u.getWorldDirection(tt);let xe=X.clone().sub(u.position).dot(tt)>0;return{x:(he.x+1)/2*le,y:(1-he.y)/2*be,visible:xe&&he.z<=1}}function ue(){for(let X of ce){let he=B(X.robot.labelAnchor(new G));if(!he.visible){X.label.style.display="none";continue}X.label.style.display="block",X.label.style.transform=`translate(${he.x.toFixed(1)}px, ${he.y.toFixed(1)}px) translate(-50%, -100%) translateY(-6px)`,X.labelName.textContent!==X.agent.name&&(X.labelName.textContent=X.agent.name);let xe=re(X);X.labelLine.textContent!==xe&&(X.labelLine.textContent=xe)}}function se(){if(!Me){U.hidden=!0,Ge="";return}let X=Me.agent;U.hidden=!1;let he=re(Me),xe=[X.name,he].join("|");xe!==Ge&&(Ge=xe,U.replaceChildren(y("header.floor-card-head",null,y("strong",null,X.name)),y("div.floor-card-activity",null,he)))}function ge(){if(!Me||U.hidden)return;let X=B(Me.robot.labelAnchor(new G)),he=Tu(X.x+24,8,Math.max(8,le-308)),xe=Tu(X.y-120,8,Math.max(8,be-260));U.style.left=`${he}px`,U.style.top=`${xe}px`}function ye(X){Me=X,Ge="",se()}function de(X){let he=a.getBoundingClientRect();st.set((X.clientX-he.left)/he.width*2-1,-((X.clientY-he.top)/he.height)*2+1),mt.setFromCamera(st,u);let xe=mt.intersectObjects(ce.map($=>$.robot.group),!0);for(let $ of xe){let Re=$.object;for(;Re;){let we=ce.find(Qe=>Qe.robot.group===Re);if(we)return we;Re=Re.parent}}return null}function Pe(X,he){return xy(X,he,pe)}let Le=()=>{let X=ee.getBoundingClientRect();le=X.width,be=X.height,!(!le||!be)&&(i.setSize(le,be,!1),u.aspect=le/be,u.updateProjectionMatrix())},vt=new ResizeObserver(()=>Le());vt.observe(ee),Le(),Z();let ot=new AbortController,Rt=(X,he)=>a.addEventListener(X,he,{signal:ot.signal}),an=null;Rt("pointerdown",X=>{an={x:X.clientX,y:X.clientY}}),Rt("pointermove",X=>{X.buttons===0&&(Ve=de(X),a.style.cursor=Ve?"pointer":"grab")}),Rt("pointerup",X=>{if(!an)return;let he=Math.hypot(X.clientX-an.x,X.clientY-an.y);an=null,he<4&&ye(de(X))}),Rt("pointerleave",()=>{Ve=null}),Rt("dblclick",X=>{let he=de(X);he&&Mn("agents",he.agent.cid)});let Gl=dt.watch(X=>{M(X),!Bt&&X.length&&(Z(),Bt=!0)}),Vl=Mt("transcript",X=>{let he=X,xe=Se.get(he.cid);switch(he.kind){case"tool_request":{let $=ar(he.data.tool)||ar(he.data.name)||"tool",Re=he.data.input??{};xe&&me(xe,Pe($,Re));let we=gy($,Re);we!==null&&H(he.cid,we,"accent");break}case"model_request":xe&&me(xe,"thinking\u2026");break;case"work_end":xe&&me(xe,"idle");break;case"work_start":{let $=he.data.origin??{};if(typeof $.from_cid=="number")H($.from_cid,he.cid,"accent");else if(he.data.kind==="task"){let Re=ce.find(we=>we.agent.tier==="orchestrator");Re&&H(Re.agent.cid,he.cid,"accent")}break}default:break}}),Ro=Mt("delta",X=>{let he=Se.get(X.cid);he&&me(he,"thinking\u2026")}),Es=Mt("conversation",X=>{let he=X;H(he.from_cid,he.to_cid,"ok")}),Wl=X=>{let he=Math.min(.05,(X-Je)/1e3);Je=X;let xe=X/1e3;L(he,X),K(X),O(xe),w(xe),d.animate(xe);for(let $ of He)$(xe);p.update(),i.render(l,u),ue(),ge()};return i.setAnimationLoop(Wl),()=>{ot.abort(),i.setAnimationLoop(null),vt.disconnect(),Gl(),Vl(),Ro(),Es(),p.dispose();for(let X of[...ce])F(X);ce=[],Se.clear();for(let X of fe.splice(0))q(X);Ir(f),Ir(b),Ir(P),Ir(m),Ir(d.group),k.dispose(),N.dispose(),T.dispose(),E.dispose(),D.dispose(),g.dispose();for(let X of Object.values(S))X.dispose();i.dispose(),n.classList.remove("floor-view"),document.body.classList.remove("floor-mode"),n.replaceChildren()}}var Rf=384,Cf=["wave","proud","think","cheer"],Pf=new Map,Ao;function yy(){let n=getComputedStyle(document.documentElement),e=(t,i)=>n.getPropertyValue(`--${t}`).trim()||i;return{ink:e("ink","#1a1a1a"),sheet:e("sheet","#ffffff"),paper:e("paper","#f5f3ee"),rule:e("rule","#d6d2c8"),signal:e("signal","#d9482b"),ok:e("ok","#2f8f5b"),info:e("info","#2f6fb0"),inkMuted:e("ink-3","#8a8a8a")}}function _y(){if(Ao!==void 0)return Ao;try{let n=document.createElement("canvas");if(!(n.getContext("webgl2")??n.getContext("webgl")))return Ao=null;let e=new ys({antialias:!0,preserveDrawingBuffer:!0});e.setSize(Rf,Rf,!1),e.shadowMap.enabled=!0;let t=new xr;t.add(new _r(15398143,9075290,1.3));let i=new br(16773590,2.6);i.position.set(2.5,4,3),i.castShadow=!0,i.shadow.mapSize.set(512,512),t.add(i);let r=new Ot({toneMapped:!1}),s=new $e(new Xn(9,9),r);s.position.set(0,1.2,-2.2),t.add(s);let o=new $e(new In(1.4,48),new js({opacity:.25}));o.rotation.x=-Math.PI/2,o.receiveShadow=!0,t.add(o);let a=new cn(30,1,.1,50);return a.position.set(1,1.25,3.4),a.lookAt(0,.72,0),Ao={renderer:e,scene:t,camera:a,backdrop:r}}catch{return Ao=null}}var If=new Map;function by(n){let e=If.get(n);if(e)return e;let t=document.createElement("canvas");t.width=t.height=512;let i=t.getContext("2d");i.fillStyle=`hsl(${n} 70% 84%)`,i.fillRect(0,0,512,512),i.fillStyle=`hsl(${n} 70% 77%)`;let r=18;for(let a=0;a<r;a++){let l=a/r*Math.PI*2,c=l+Math.PI/r;i.beginPath(),i.moveTo(256,300),i.lineTo(256+Math.cos(l)*800,300+Math.sin(l)*800),i.lineTo(256+Math.cos(c)*800,300+Math.sin(c)*800),i.closePath(),i.fill()}let s=i.createRadialGradient(256,300,10,256,300,260);s.addColorStop(0,"rgba(255,255,255,0.75)"),s.addColorStop(1,"rgba(255,255,255,0)"),i.fillStyle=s,i.fillRect(0,0,512,512);let o=new Ii(t);return o.colorSpace=Pt,If.set(n,o),o}function kf(n){let e=`${n.cid}|${n.roleHue}|${Math.round(n.bodyHue)}|${n.orchestrator}|${n.state==="offline"}`,t=Pf.get(e);if(t)return t;let i=_y();if(!i)return null;let r=Il(n.roleHue,n.orchestrator,yy(),n.bodyHue);r.setState(n.state==="offline"?"offline":"idle"),r.pose(Cf[n.cid%Cf.length]),r.group.rotation.y=-.35,r.group.traverse(o=>o.castShadow=!0),n.orchestrator&&r.group.scale.multiplyScalar(.85),i.backdrop.map=by(Math.round(n.bodyHue)),i.backdrop.needsUpdate=!0,i.scene.add(r.group),i.renderer.render(i.scene,i.camera);let s=i.renderer.domElement.toDataURL("image/png");return i.scene.remove(r.group),r.dispose(),Pf.set(e,s),s}function Lf(n,e=40){let t=Au(n.cid),i=kf({cid:n.cid,roleHue:n.tier==="orchestrator"?25:Ru(n.role),bodyHue:t,orchestrator:n.tier==="orchestrator",state:n.status==="offline"?"offline":"idle"}),r=y("span.avatar",{style:`width:${e}px;height:${e}px;--hue:${t}`});return i?r.append(y("img",{src:i,alt:"",width:String(e),height:String(e)})):r.append(y("span.avatar-initial",null,ln(n).slice(0,1).toUpperCase())),n.status==="offline"&&r.classList.add("offline"),r}var My={working:"info",idle:"ok",offline:"muted"},Sy={task:"accent",chat:"info",message:"muted",motion:"warn"},Cu=16;function Df(n){let e="",t="all",i=[],r=gn([{value:"",label:"All roles"}]),s=gn([{value:"all",label:"All statuses"},{value:"working",label:"Working"},{value:"idle",label:"Idle"},{value:"offline",label:"Offline"}]),o=y("div"),a=Ze("Spawn agents",()=>x(),{kind:"primary",icon:"plus"});r.addEventListener("change",()=>{e=r.value,d(dt.get())}),s.addEventListener("change",()=>{t=s.value,d(dt.get())});let l=[{label:"Agent",cell:v=>y("div.row.agent-cell",null,Lf(v,40),y("div",null,y("div.row",null,mi(My[v.status]),y("strong.truncate",null,ln(v))),y("div.faint.truncate",null,xn(v.role))))},{label:"Model",cell:v=>y("div.row.agent-model",null,y("span.mono.truncate",null,v.model),v.tier==="orchestrator"?at("orchestrator","accent"):null,v.model_source!=="default"?at(v.model_source==="agent"?"own":"role"):null)},{label:"Work",cell:v=>v.work?y("div.row.agent-work",null,at(v.work.kind,Sy[v.work.kind]),y("span.truncate",null,Qt(v.work.prompt,56))):y("span.faint",null,"\u2014")},{label:"Tokens",num:!0,cell:v=>Ur(v.usage.input_tokens+v.usage.output_tokens)},{label:"",width:"1%",cell:v=>h(v)}];n.append(y("div.view-head",null,y("h1",null,"Agents"),y("div.row.agent-filters",null,r,s,a)),Xt(null,o,{flush:!0}));let c=dt.watch(d);return()=>c();function h(v){return y("div.row.agent-actions",null,Ze("",m=>{m.stopPropagation(),Mn("agents",v.cid)},{kind:"ghost",size:"sm",icon:"conversations",title:"Chat"}),v.status==="working"?Ze("",m=>{m.stopPropagation(),pt(()=>ze.cancel(v.cid),"Work cancelled")},{kind:"ghost",size:"sm",icon:"ban",title:"Cancel work"}):null,Ze("",m=>{m.stopPropagation(),pt(()=>ze.stop(v.cid),"Session stopped")},{kind:"ghost",size:"sm",icon:"stop",title:"Stop session"}))}function d(v){if(u(v),!v.length){Be(o,ft("agents","No agents connected","Spawn agents to fill the roster.",Ze("Spawn agents",()=>x(),{kind:"primary",icon:"plus"})));return}let m=v.filter(f=>(e===""||f.role===e)&&(t==="all"||f.status===t));if(!m.length){Be(o,ft("search","No agents match","Clear the filters to see the roster.",Ze("Clear filters",p,{kind:"ghost"})));return}Be(o,y("div.table-scroll",null,ur(l,m,f=>Mn("agents",f.cid))))}function u(v){let m=[...new Set(v.map(f=>f.role))].sort();m.join(`
`)!==i.join(`
`)&&(i=m,e&&!i.includes(e)&&(e=""),r.replaceChildren(new Option("All roles",""),...i.map(f=>new Option(xn(f),f))),r.value=e)}function p(){e="",t="all",r.value="",s.value="all",d(dt.get())}function x(){let v=gn(i.map(T=>({value:T,label:T}))),m=jt({type:"number",min:1,max:Cu,value:1}),f=gn([{value:"",label:"Default for the role"}]),b=Ze("Spawn",()=>{w()},{kind:"primary",icon:"plus"}),P=()=>{},w=async()=>{let T=Math.trunc(Number(m.value));if(!v.value){sn("Pick a role","error");return}if(!(T>=1&&T<=Cu)){sn(`Count must be 1 to ${Cu}`,"error");return}b.disabled=!0;let E=await pt(()=>ze.spawnAgents(v.value,T,f.value||void 0),"Spawning\u2026");b.disabled=!1,E!==void 0&&(P(),lr())};pt(()=>ze.models()).then(T=>{T&&f.append(...T.models.map(E=>new Option(`${E.provider}/${E.id}`,`${E.provider}/${E.id}`)))}),P=Ln("Spawn agents",y("div.stack",null,yt("Role",v),yt("Count",m),yt("Model",f),y("div.row.modal-actions",null,b)))}}function Nf(n,e){let t=Number(e[0]);return e[0]&&Number.isFinite(t)?Qu(n,t):Df(n)}var wy=300,Uf="3";function Ff(n,e){let t=y("select.board-filter",{"aria-label":"Filter by author"}),i=y("input.board-search",{type:"search",placeholder:"Search subject or body","aria-label":"Search the board"}),r=y("div.board-feed");n.append(y("div.view-head",null,y("h1",null,"Board"),y("div.actions.board-actions",null,t,i)),r);let s=[],o=!1,a=!1,l=!0,c,h="",d="",u=P=>dt.get().filter(w=>w.cid===P.agent_cid).map(ln)[0]??`agent ${P.agent_cid}`,p=()=>{let P=new Map;for(let T of s){let E=String(T.agent_cid);P.has(E)||P.set(E,u(T))}let w=Array.from(P).sort((T,E)=>T[1].localeCompare(E[1]));h&&!P.has(h)&&(h=""),Be(t,y("option",{value:""},"All authors"),w.map(([T,E])=>y("option",{value:T},E))),t.value=h},x=()=>{for(let P of r.querySelectorAll(".board-body")){let w=P.nextElementSibling;w instanceof HTMLElement&&P.scrollHeight>P.clientHeight+1&&(w.hidden=!1)}},v=()=>{if(!o)return;if(a)return Be(r,ft("alert","Could not load the board."));if(p(),!s.length)return Be(r,ft("board","The board is empty."));let P=[...s].sort((E,D)=>D.created_at-E.created_at),w=d.trim().toLowerCase(),T=P.map((E,D)=>({p:E,no:P.length-D})).filter(({p:E})=>!h||String(E.agent_cid)===h).filter(({p:E})=>!w||`${E.subject}
${E.body}`.toLowerCase().includes(w));if(!T.length)return Be(r,ft("board","Nothing on the board matches."));Be(r,T.map(({p:E,no:D})=>Ey(E,D,u(E)))),x()},m=async()=>{try{s=await ze.board(),a=!1}catch{a=!0}o=!0,l&&v()},b=[Mt("board",()=>{window.clearTimeout(c),c=window.setTimeout(()=>{m()},wy)}),dt.watch(()=>v())];return t.addEventListener("change",()=>{h=t.value,v()}),i.addEventListener("input",()=>{d=i.value,v()}),m(),()=>{l=!1,window.clearTimeout(c),b.forEach(P=>P())}}function Ey(n,e,t){let i=n.body.trim(),r=i&&i!==n.subject.trim()?y("p.board-body",null,i):null;r&&r.style.setProperty("-webkit-line-clamp",Uf);let s=y("button.board-more",{type:"button",hidden:!0,onclick:()=>{if(!r)return;let o=r.classList.toggle("open");r.style.setProperty("-webkit-line-clamp",o?"none":Uf),s.textContent=o?"Show less":"Show more"}},"Show more");return y("article.board-entry",null,y("span.board-no",null,`p${e}`),y("div.board-matter",null,y("div.board-by",null,y("a.board-author",{href:bn("agents",n.agent_cid)},t),y("span.board-role",null,xn(n.role))),y("h3.board-subject",null,n.subject),r,r?s:null),y("time.board-time",null,qt(n.created_at)))}var Ty=300;function Of(n,e){let t=y("div.convo-list"),i=y("div.thread");n.append(y("div.view-head",null,y("h1",null,"Conversations")),y("div.convo",null,t,i));let r=[],s=!1,o=!1,a=!0,l,c=e[0]?Ay(e[0]):void 0,h=m=>{let f=dt.get().find(b=>b.cid===m);return f?ln(f):`agent ${m}`},d=m=>{c=m,u(),Mn("conversations",m)},u=()=>{if(!s)return;if(o){Be(t,ft("alert","Could not load conversations.")),Be(i);return}let m=Ry(r);if(!m.length){Be(t,ft("conversations","No conversations yet.")),Be(i);return}let f=m.find(b=>b.key===c)??m[0];f&&(c=f.key,Be(t,m.map(b=>Cy(b,b.key===f.key,h,()=>d(b.key)))),Be(i,...Py(f,h)),i.scrollTop=i.scrollHeight)},p=async()=>{try{r=await ze.conversations(),o=!1}catch{o=!0}s=!0,a&&u()},v=[Mt("conversation",()=>{window.clearTimeout(l),l=window.setTimeout(()=>{p()},Ty)}),dt.watch(()=>u())];return p(),()=>{a=!1,window.clearTimeout(l),v.forEach(m=>m())}}function Ay(n){return n.split("-").map(Number).sort((e,t)=>e-t).join("-")}function Ry(n){let e=new Map,t=[...n].sort((i,r)=>i.created_at-r.created_at);for(let i of t){let r=Math.min(i.from_cid,i.to_cid),s=Math.max(i.from_cid,i.to_cid),o=`${r}-${s}`,a=e.get(o)??{key:o,a:r,b:s,messages:[],last:i};a.messages.push(i),a.last=i,e.set(o,a)}return Array.from(e.values()).sort((i,r)=>r.last.created_at-i.last.created_at)}function Pu(n,e){return y("a.agent-link",{href:bn("agents",n)},e(n))}function Cy(n,e,t,i){let r=y("button.convo-item",{type:"button","aria-current":e?"true":null},y("span.convo-names",null,`${t(n.a)} \xB7 ${t(n.b)}`),y("span.convo-last",null,Qt(n.last.text,90)),y("span.convo-meta",null,`${qt(n.last.created_at)} \xB7 ${n.messages.length} messages`));return r.addEventListener("click",i),r}function Py(n,e){let t=n.messages[0]?.from_cid??n.a;return[y("div.thread-head",null,Pu(n.a,e)," \xB7 ",Pu(n.b,e)),...n.messages.map(i=>Iy(i,i.from_cid===t?"left":"right",e))]}function Iy(n,e,t){return y("div.bubble-row",{class:e},y("div.bubble",null,y("div.bubble-meta",null,Pu(n.from_cid,t),y("span.faint",null,qt(n.created_at)),n.delivered?null:at("undelivered","warn")),y("p.bubble-text",null,n.text)))}var Hf=n=>/owner|await/i.test(n.state);function ky(n){return Hf(n)?"warn":/carr|pass|approv/i.test(n.state)?"ok":/reject|fail|den/i.test(n.state)?"danger":/open/i.test(n.state)?"info":"muted"}var Ly={yes:"ok",no:"danger",abstain:"muted"},Iu=(n,e)=>`${e>0?n/e*100:0}%`;function zf(n,e){let t=y("div.stack");n.append(y("div.view-head",null,y("h1",null,"Council")),t);let i=!0,r=async()=>{try{let o=await ze.motions();if(!i)return;Be(t,o.length?o.map(a=>Dy(a,()=>{r()})):ft("council","No motions."))}catch(o){i&&Be(t,ft("alert","Council unreachable",o instanceof Error?o.message:String(o)))}},s=Mt("motion",()=>{r()});return r(),()=>{i=!1,s()}}function Dy(n,e){let t=n.tally,i=y("div.tally",{role:"img","aria-label":`${t.yes} yes, ${t.no} no, ${t.abstain} abstain of ${t.seats}`},y("span.tally-seg.yes",{style:`width:${Iu(t.yes,t.seats)}`}),y("span.tally-seg.no",{style:`width:${Iu(t.no,t.seats)}`}),y("span.tally-seg.abstain",{style:`width:${Iu(t.abstain,t.seats)}`})),r=n.votes?.length?y("ul.votes",null,n.votes.map(o=>y("li",null,y("span.muted",null,xn(o.role)),at(o.vote,Ly[o.vote]??"muted"),y("span",null,o.reason)))):null,s=Hf(n)?y("div.motion-actions",null,Ze("Reject",()=>Bf(n,"reject",e),{kind:"danger",icon:"x"}),Ze("Approve",()=>Bf(n,"approve",e),{kind:"primary",icon:"check"})):null;return Xt(y("div.motion-head",null,at(n.kind,"accent"),at(n.state,ky(n),!0),y("span.faint",null,qt(n.created_at))),y("div.stack",null,y("p.motion-text",null,n.text),i,y("div.tally-legend.faint",null,`${t.yes} yes \xB7 ${t.no} no \xB7 ${t.abstain} abstain \xB7 ${t.voted}/${t.seats}`),r,s))}function Bf(n,e,t){let i=e==="approve"?"Approve":"Reject",r=ii({rows:3,placeholder:"Reason"}),s=Ze(i,()=>{s.disabled=!0,pt(()=>ze.decide(n.id,e,r.value.trim()),`Motion ${e==="approve"?"approved":"rejected"}`).then(a=>{a?(o(),t()):s.disabled=!1})},{kind:e==="approve"?"primary":"danger"}),o=Ln(`${i} motion`,y("div.stack",null,y("p.muted",null,n.text),r,y("div.motion-actions",null,Ze("Cancel",()=>o(),{kind:"ghost"}),s)))}var Gf="http://www.w3.org/2000/svg";function Zt(n,e,t={}){let i=document.createElementNS(Gf,e);for(let r of Object.keys(t))i.setAttribute(r,String(t[r]));return n.appendChild(i),i}function $n(n,e,t,i,r,s="var(--sheet)",o={}){return Zt(n,"rect",{x:e,y:t,width:i,height:r,fill:s,...o})}function Fl(n,e,t){e.forEach(([i,r],s)=>{let o=Zt(n,"circle",{cx:i,cy:r,r:1.6,fill:t?"var(--ok)":"var(--paper)","stroke-width":1});t&&s===0&&Zt(o,"animate",{attributeName:"opacity",values:"1;0.15;1",dur:"1.6s",repeatCount:"indefinite"})})}function Ny(n,e){$n(n,8,18,68,44),$n(n,13,23,40,14,"var(--paper)");for(let t of[17,21,25])Zt(n,"line",{x1:t,y1:27,x2:t,y2:33});$n(n,13,41,40,15,"var(--paper)");for(let t of[45,48,51,54])Zt(n,"line",{x1:17,y1:t,x2:49,y2:t,"stroke-width":1}),Zt(n,"line",{x1:58,y1:t,x2:73,y2:t,"stroke-width":1});return Fl(n,[[58,30],[63,30],[68,30],[73,30]],e),76}function Uy(n,e){$n(n,26,8,42,76),Zt(n,"polygon",{points:"68,8 76,3 76,79 68,84",fill:"var(--paper)"});for(let t of[30,36,42])Zt(n,"line",{x1:70,y1:t-1.25,x2:75,y2:t-4.4,"stroke-width":1});$n(n,32,14,30,18,"var(--paper)"),Zt(n,"line",{x1:36,y1:22,x2:58,y2:22,"stroke-width":1}),Zt(n,"line",{x1:36,y1:26,x2:58,y2:26,"stroke-width":1});for(let t of[70,73,76,79])Zt(n,"line",{x1:34,y1:t,x2:62,y2:t,"stroke-width":1});return Zt(n,"circle",{cx:52,cy:60,r:3.5,fill:"var(--paper)"}),Fl(n,[[36,60],[42,60]],e),76}function Fy(n,e){let t="#5a9a78";$n(n,10,30,70,40,"var(--sheet)",{stroke:t}),[[14,34],[76,34],[14,66],[76,66]].forEach(([r,s])=>{Zt(n,"circle",{cx:r,cy:s,r:1.5,stroke:t,"stroke-width":1})}),$n(n,30,40,20,20,"var(--paper)"),Zt(n,"rect",{x:36,y:46,width:8,height:8,"stroke-width":1});for(let r=0;r<10;r++){let s=52+r*2.2;Zt(n,"circle",{cx:s,cy:34,r:.9,"stroke-width":.8}),Zt(n,"circle",{cx:s,cy:37,r:.9,"stroke-width":.8})}return $n(n,69,48,11,9,"var(--paper)"),$n(n,69,58,11,9,"var(--paper)"),$n(n,54,60,13,10,"var(--paper)"),Fl(n,[[18,62],[23,62]],e),80}function Oy(n,e){$n(n,10,30,66,30,"var(--sheet)",{rx:8}),Zt(n,"line",{x1:18,y1:35,x2:66,y2:35,"stroke-width":1});for(let t of[50,53])Zt(n,"line",{x1:20,y1:t,x2:50,y2:t,"stroke-width":1});return Fl(n,[[66,45]],e),76}var By={server:Ny,workstation:Uy,edge:Fy,apple:Oy};function Hy(n,e){let t=Zt(n,"g");Zt(t,"line",{x1:e,y1:46,x2:86,y2:46}),$n(t,86,36,28,24),Zt(t,"circle",{cx:98,cy:46,r:6}),Zt(t,"circle",{cx:98,cy:46,r:1.5}),$n(t,90,56,20,3,"var(--gold, #c9a227)",{stroke:"none"})}function Vf(n,e){let t=document.createElementNS(Gf,"svg");t.setAttribute("viewBox","0 0 120 90"),t.setAttribute("class","device"),t.setAttribute("role","img"),t.setAttribute("aria-label",n),t.setAttribute("fill","none"),t.setAttribute("stroke","currentColor"),t.setAttribute("stroke-width","1.5");let i=By[n](t,e.active);return e.gpu&&Hy(t,i),t}var zy="http://www.w3.org/2000/svg",Gy=2e3,$f={server:"Server",workstation:"Workstation",edge:"Edge",apple:"Apple"},Wf=[["Run",!1],["Role",!1],["State",!1],["VMs",!0],["Started",!0],["",!1]];function Lu(n){return/run|ready|up|active/i.test(n)?"ok":/start|boot|pend|creat|launch/i.test(n)?"info":/fail|err|crash|kill|dead/i.test(n)?"danger":"muted"}var Yf=n=>n.some(e=>e.vms.some(t=>Lu(t.state)==="ok")),Vy=n=>(n.runs??[]).reduce((e,t)=>e+t.vms.length,0),ku=n=>(n/1024).toFixed(1),Zf=(n,e,t)=>Math.min(t,Math.max(e,n));function Xf(n,e){let t=document.createElementNS(zy,n);for(let[i,r]of Object.entries(e))t.setAttribute(i,r);return t}function Wy(n){if(n<60)return`${Math.floor(n)}s`;let e=Math.floor(n/60);if(e<60)return`${e}m`;let t=Math.floor(e/60);return t<48?`${t}h ${e%60}m`:`${Math.floor(t/24)}d ${t%24}h`}function Xy(n){let e=`${$f[n.kind]} \xB7 ${n.cpus}-core ${n.arch} on ${n.backend.toUpperCase()} \xB7 ${Math.round(n.memory_mib/1024)} GiB memory`;return n.gpus.length?`${e} \xB7 ${n.gpus.map(t=>t.name).join(", ")}`:e}function qy(n){let i=document.createElement("canvas");i.className="mach-spark",i.width=240,i.height=72,i.setAttribute("aria-hidden","true");let r=i.getContext("2d");if(!r)return i;let s=getComputedStyle(document.documentElement);r.strokeStyle=s.getPropertyValue("--rule").trim()||"#999",r.lineWidth=1,r.beginPath(),r.moveTo(0,72-.5),r.lineTo(240,72-.5),r.stroke();let o=n.slice(-60);return o.length<2||(r.strokeStyle=s.getPropertyValue("--ink").trim()||"#111",r.lineWidth=2,r.beginPath(),o.forEach((a,l)=>{let c=l/(o.length-1)*240,h=68-Zf(a.cpu_percent,0,100)/100*64;l===0?r.moveTo(c,h):r.lineTo(c,h)}),r.stroke()),i}function $y(n){let e=y("div.mach-bar-fill",null);return e.style.width=`${Zf(n.mem_used_mib/Math.max(n.mem_total_mib,1)*100,0,100)}%`,y("div.mach-bar",null,e)}function Ol(n,e,t,i){let r=y("div.mach-figure",null,e);return e.length>8&&r.classList.add("long"),y("div.mach-tally",null,y("div.mach-tally-label",null,n),r,i??null,t===null?null:y("div.mach-caption",null,t))}function qf(n,e,t,i){let r=Xf("svg",{class:"mach-wires",viewBox:"0 0 100 100",preserveAspectRatio:"none","aria-hidden":"true"}),s=y("div.mach-wires-box",null,r),o=y("div.mach-fleet");o.style.gridTemplateRows=`repeat(${Math.max(e.length,1)}, minmax(44px, 1fr))`,e.length||o.append(ft("machine","No machines.")),e.forEach((l,c)=>{let h=Yf(l.runs??[]),d=(c+.5)/e.length*100;r.append(Xf("line",{x1:"0",y1:"50",x2:"100",y2:String(d),class:h?"mach-wire live":"mach-wire","vector-effect":"non-scaling-stroke"}));let u=y("div.mach-wire-label",null,`${l.agents??0} agents \xB7 ${Vy(l)} VMs`);u.style.top=`${(50+d)/2}%`,s.append(u);let p=y("span.mach-mark",null);h&&p.classList.add("live"),l.reachable===!1&&p.classList.add("down");let x=l.reachable===!1?"unreachable":h?"running":"idle",v=y("div.mach-block",{role:"button",tabindex:"0"},y("div.mach-row",null,p,y("span.mach-name",null,l.name)),y("div.faint",null,`${$f[l.kind]} \xB7 ${x}`));v.classList.toggle("selected",l.id===t),v.addEventListener("click",()=>i(l.id)),v.addEventListener("keydown",m=>{m.key==="Enter"&&i(l.id)}),o.append(v)});let a=y("div.mach-plane",null,y("div.mach-caps",null,"Control plane"),y("div.faint",null,n?`${n.agents.total} agents \xB7 ${n.agents.working} working`:"Unreachable"),y("div.mach-chips",null,at("apid",n?"ok":"danger",!0),at("hub",n?.hub?"ok":"muted",!0)));return y("div.mach-topo",null,a,s,o)}function Kf(n,e){let t=y("div"),i=y("div");n.append(y("div.view-head",null,y("h1",null,"Machines")),t,i);let r=new Set,s=new Set,o=new Set,a=new Map,l=!0,c,h=[],d=null,u=null,p=(g,S)=>S.length?y("div.vms",null,S.map(_=>{let A=`${g}/${_.index}`,I=null;if(_.serial){let k=y("details",null,y("summary",null,"Serial"),y("pre.pre",null,_.serial));k.open=s.has(A),k.addEventListener("toggle",()=>k.open?s.add(A):s.delete(A)),I=k}return y("div.vm",null,y("div.row",null,y("strong",null,`VM ${_.index}`),at(_.state,Lu(_.state),!0),y("span.faint",null,`guest CID ${_.guest_cid??"\u2014"}`)),I)})):y("p.faint",null,"No VMs."),x=g=>{let S=a.get(g.id),_=y("td",{colspan:Wf.length},S?p(g.id,S.vms):y("p.faint",null,"Loading\u2026")),A=y("tr.detail",null,_);return ze.run(g.id).then(I=>{a.set(g.id,I),A.isConnected&&Be(_,p(g.id,I.vms))},I=>{A.isConnected&&Be(_,y("p.faint",null,I instanceof Error?I.message:String(I)))}),A},v=(g,S)=>{let _=S.nextElementSibling;_&&_.classList.contains("detail")?(_.remove(),r.delete(g.id)):(r.add(g.id),S.after(x(g)))},m=async g=>{await pt(()=>ze.killRun(g),`Killed run ${g}`),l&&w()},f=g=>{let S=y("tbody");for(let _ of g){let A=Ze("Kill",k=>{k.stopPropagation(),m(_.id)},{kind:"danger",size:"sm",icon:"stop"}),I=y("tr.clickable",null,y("td",null,_.id),y("td",null,_.role??"fleet"),y("td",null,at(_.state,Lu(_.state),!0)),y("td",{class:"num"},`${_.vms.length} VMs`),y("td",{class:"num"},`started ${qt(_.created_at)}`),y("td",{class:"num"},A));I.addEventListener("click",()=>v(_,I)),S.append(I),r.has(_.id)&&S.append(x(_))}return y("table.table.mach-runs",null,y("thead",null,y("tr",null,Wf.map(([_,A])=>y("th",{class:A?"num":""},_)))),S)},b=g=>{let S=g.runs??[],_=Vf(g.kind,{gpu:g.gpus.length>0,active:Yf(S)});_.classList.add("mach-dev");let A=g.stats,I=A?y("div.mach-live",null,y("div.mach-tallies",null,Ol("CPU",`${Math.round(A.cpu_percent)}%`,null,qy(A.history)),Ol("Memory",`${ku(A.mem_used_mib)} / ${ku(A.mem_total_mib)}`,"GiB used of GiB",$y(A)),Ol("Load",`${A.load[0].toFixed(1)} \xB7 ${A.load[1].toFixed(1)} \xB7 ${A.load[2].toFixed(1)}`,"1 / 5 / 15 min"),Ol("Agents' VMs",String(A.vms_running),`${ku(A.guest_mem_mib)} GiB reserved`)),y("p.faint",null,`Up ${Wy(A.uptime_s)}`)):y("p.mach-none",null,"No live stats."),k=y("dl.kv"),N=[["Class",g.class],["Arch",g.arch],["Backend",g.backend],["Irqchip",g.irqchip],["vsock",g.vsock_mode],["noded port",g.noded_port===void 0?"\u2014":String(g.noded_port)]];for(let[z,Y]of N)k.append(y("dt",null,z),y("dd",null,Y));let U=y("details.mach-tech",null,y("summary",null,"Technical details"),k);return U.open=o.has(g.id),U.addEventListener("toggle",()=>U.open?o.add(g.id):o.delete(g.id)),y("section.mach-section",{id:`mach-${g.id}`},y("div.mach-figure-box",null,_),y("div.mach-body",null,y("h2.mach-heading",null,g.name),y("p.mach-sentence",null,Xy(g)),I,y("div.mach-caps",null,"Runs"),S.length?f(S):y("p.faint",null,"No runs."),U))},P=g=>{u=g,t.replaceChildren(qf(d,h,u,P)),document.getElementById(`mach-${g}`)?.scrollIntoView({behavior:"smooth",block:"start"})},w=async()=>{try{let[g,S]=await Promise.all([ze.overview().catch(()=>null),ze.machines()]);if(!l)return;d=g,h=S,S.some(A=>A.id===u)||(u=S[0]?.id??null);let _=new Set(S.flatMap(A=>(A.runs??[]).map(I=>I.id)));for(let A of Array.from(r))_.has(A)||r.delete(A);t.replaceChildren(qf(g,S,u,P)),i.replaceChildren(...S.length?S.map(b):[ft("machine","No machines.")])}catch(g){if(!l)return;t.replaceChildren(),i.replaceChildren(ft("alert","Machines unreachable",g instanceof Error?g.message:String(g)))}},T=()=>{c!==void 0&&clearTimeout(c),c=setTimeout(()=>{c=void 0,w()},500)},E=[Mt("run",T),Mt("vm",T)],D=setInterval(()=>{w()},Gy);return w(),()=>{l=!1,E.forEach(g=>g()),clearInterval(D),c!==void 0&&clearTimeout(c)}}var Yy=["default","low","medium","high","xhigh","max"],Jf="models-policy-known";function jf(n){let e=!0;return Be(n,y("p.faint",null,"Loading\u2026")),pt(()=>Promise.all([ze.settings(),ze.models()])).then(t=>{if(e){if(!t){Be(n,ft("alert","Models unavailable","apid did not answer. Reload to try again."));return}Zy(n,t[0],t[1])}}),()=>{e=!1}}function Zy(n,e,t){let i=e,r=Uu(e),s=e.dev_models===!0,o=new Set(t.models.map(g=>`${g.provider}/${g.id}`)),a=new Set,l=!1,c=Ze("Save",()=>{x()},{kind:"primary",icon:"check"}),h=y("datalist",{id:Jf},...t.models.map(g=>y("option",{value:`${g.provider}/${g.id}`},g.name??`${g.provider}/${g.id}`))),d=y("div.stack.models-policy");Be(n,y("div.policy-bar",null,c),h,d);function u(){c.disabled=a.size>0||JSON.stringify(r)===JSON.stringify(i)}function p(){a.clear(),Be(d,m(),P(),T(),D()),u()}async function x(){c.disabled=!0;let g=await pt(()=>ze.saveSettings(r),"Policy saved");g?(i=Uu(g),r=Uu(g),p()):u()}function v(g,S,_){_?a.add(S):a.delete(S),g.classList.toggle("invalid",_),u()}function m(){return Xt("Model policy",y("div.stack",null,y("p.policy-lead",null,"Agents may only run the models on their tier's list."),y("div.policy-cols",null,f("orchestrator","Orchestrator"),f("worker","Workers"))),{actions:s?[at("Development models enabled \u2014 policy relaxed","warn")]:[]})}function f(g,S){let A=r.model_policy[g].map((N,U)=>b(g,N,U)),I=jt({placeholder:"provider/model",list:Jf,autocomplete:"off","aria-label":`${S}: model to add`}),k=()=>{let N=I.value.trim();if(N!==""){if(!/^[^/\s]+\/\S+$/.test(Du(N).base)){sn("Use provider/model, with an optional @effort","error");return}r.model_policy[g].includes(N)||(r.model_policy[g]=[...r.model_policy[g],N]),p()}};return I.addEventListener("keydown",N=>{N.key==="Enter"&&(N.preventDefault(),k())}),y("section.policy-col",null,y("h3",null,S),y("ul.policy-list",null,...A.length?A:[y("li.faint",null,"No models")]),y("div.policy-add",null,I,Ze("Add model",k,{size:"sm"})))}function b(g,S,_){let{base:A,effort:I}=Du(S),k=o.has(A),N=mi(k?"ok":"muted");N.title=k?"Listed by its provider":"Not listed by its provider";let U=I===null?null:at("effort pinned","accent");U&&(U.title=`@${I}`);let z=Ze("",()=>{r.model_policy[g]=r.model_policy[g].filter((Y,ee)=>ee!==_),p()},{kind:"ghost",size:"sm",icon:"x",title:`Remove ${S}`});return y("li.policy-row",null,N,y("span.mono",{title:S},A),U,z)}function P(){let g=gn(Nu(r.model_policy.orchestrator,r.orchestrator_model),r.orchestrator_model);g.addEventListener("change",()=>{r.orchestrator_model=g.value,u()});let S=gn(Nu(r.model_policy.worker,r.default_model),r.default_model);S.addEventListener("change",()=>{r.default_model=S.value,u()});let _=Array.from(new Set([...dt.get().filter(I=>I.tier!=="orchestrator"&&I.role!=="orchestrator").map(I=>I.role),...Object.keys(r.role_models)])).sort(),A=_.length?ur([{label:"Role",cell:I=>y("span.mono",null,I)},{label:"Model",cell:I=>w(I)}],_):ft("agents","No roles connected");return Xt("Defaults",y("div.stack",null,yt("Orchestrator model",g),yt("Worker default",S),y("h3",null,"Per role"),A))}function w(g){let S=r.role_models[g]??"",_=gn(Nu(r.model_policy.worker,S===""?null:S,{value:"",label:"Worker default"}),S);return _.addEventListener("change",()=>{_.value===""?delete r.role_models[g]:r.role_models[g]=_.value,u()}),_}function T(){let g=E("max_turns",1,200,r.max_turns,I=>{r.max_turns=I}),S=E("max_tokens",0,1e9,r.max_tokens,I=>{r.max_tokens=I}),_=gn(Yy.map(I=>({value:I,label:I})),r.effort??"default");_.addEventListener("change",()=>{r.effort=_.value==="default"?null:_.value,u()});let A=ii({rows:5,spellcheck:"false",placeholder:"api.example.com"});return A.value=r.fetch_allow.join(`
`),A.addEventListener("input",()=>{r.fetch_allow=A.value.split(`
`).map(I=>I.trim()).filter(I=>I!==""),u()}),Xt("Limits",y("div.stack",null,yt("Max turns",g),yt("Max tokens (0 = provider default)",S),yt("Effort",_),yt("Fetch allowlist",A)))}function E(g,S,_,A,I){let k=jt({type:"number",min:S,max:_,step:1,value:A});return k.addEventListener("input",()=>{let N=Number(k.value),U=k.value.trim()!==""&&Number.isInteger(N)&&N>=S&&N<=_;U&&I(N),v(k,g,!U)}),k}function D(){let g=r.model_policy.orchestrator.concat(r.model_policy.worker).map(k=>Du(k).base),S=Array.from(new Set([...g,...Object.keys(r.prices)])).sort(),_=(k,N)=>{let U=r.prices[k],z=jt({type:"number",min:0,step:"any",value:U?U[N]:0});return z.addEventListener("input",()=>{let Y=Number(z.value),ee=z.value.trim()!==""&&Number.isFinite(Y)&&Y>=0;if(ee){let J=r.prices[k]??{input:0,output:0};J[N]=Y,r.prices[k]=J}v(z,`price:${k}:${N}`,!ee)}),z},A=S.length?ur([{label:"Model",cell:k=>y("span.mono",null,k)},{label:"Input (credits / 1M)",num:!0,cell:k=>_(k,"input")},{label:"Output (credits / 1M)",num:!0,cell:k=>_(k,"output")}],S):ft("models","No models listed"),I=y("details.prices",null,y("summary",null,"Models"),A);return I.open=l,I.addEventListener("toggle",()=>{l=I.open}),Xt("Prices",I)}p()}function Du(n){let e=n.lastIndexOf("@");return e>0?{base:n.slice(0,e),effort:n.slice(e+1)}:{base:n,effort:null}}function Nu(n,e,t){let r=(e!==null&&e!==""&&!n.includes(e)?[...n,e]:n).map(s=>({value:s,label:n.includes(s)?s:`${s} (not on the list)`}));return t?[t,...r]:r}function Uu(n){return JSON.parse(JSON.stringify(n))}var Ky=["Configured with a key","Needs a key","Local"],Jy={env:["Key set","ok"],secret:["Key set","ok"],none:["No key needed","muted"],command:["Command","info"],missing:["Key missing","warn"]};function Qf(n,e){return e.find(t=>t.id===n.id)??e.find(t=>t.kind===n.kind)}function jy(n){return n.credential==="missing"?"Needs a key":n.credential==="none"?"Local":"Configured with a key"}function Qy(n){return n.split(",").map(e=>e.trim()).filter(Boolean)}function ep(n){let e=[],t=[],i=y("div.prov-body",null,y("p.faint",null,"Loading providers\u2026")),r=Ze("Add provider",()=>x(),{kind:"primary"});n.append(y("div.prov-bar",null,r),i);let s=async()=>{let[v,m]=await Promise.all([pt(()=>ze.providerKinds()),pt(()=>ze.providers())]);v&&(e=v),m&&(t=m),o()},o=()=>{if(!t.length){Be(i,ft("models","No providers yet"));return}let v={"Configured with a key":[],"Needs a key":[],Local:[]};for(let f of t)v[jy(f)].push(f);let m=Ky.filter(f=>v[f].length>0).map(f=>y("section.prov-group",null,y("h2.prov-group-title",null,f),y("div.prov-grid",null,v[f].map(a))));Be(i,...m)},a=v=>{let m=Qf(v,e),[f,b]=Jy[v.credential],P=y("div.prov-models");P.hidden=!0;let w=!1,T=Ze("",async()=>{if(P.hidden=!P.hidden,P.hidden||w)return;let E=await pt(()=>ze.providerModels(v.id));E&&(w=!0,Be(P,E.length?y("ul.prov-model-list",null,E.map(D=>y("li.mono",null,D.id))):y("p.faint",null,"No models listed")))},{size:"sm",kind:"ghost",icon:"models",title:"Models"});return y("article.prov-card",null,y("header.prov-head",null,Jl(m?.brand??"mock",!0),y("div.prov-title",null,y("h3.truncate",null,m?.label??v.id),y("span.faint.mono.truncate",{title:v.base_url??""},`${v.id} \xB7 ${v.base_url??"\u2014"}`))),y("div.prov-meta",null,at(f,b,!0),y("span.faint",null,`${v.models.length} ${v.models.length===1?"model":"models"}`)),y("div.prov-actions",null,Ze("Test",()=>{l(v)},{size:"sm"}),Ze(v.credential==="missing"?"Add key":"Key",()=>c(v),{size:"sm"}),y("span.prov-spacer"),T,Ze("",()=>u(v),{size:"sm",kind:"ghost",icon:"edit",title:"Edit"}),Ze("",()=>h(v),{size:"sm",kind:"ghost",icon:"trash",title:"Remove"})),P)},l=async v=>{let m=await pt(()=>ze.testProvider(v.id));m&&(m.ok?sn(`OK \xB7 ${m.latency_ms} ms \xB7 ${m.model}`):sn(m.error??"Test failed","error"))},c=v=>{let m=()=>{},f=jt({type:"password",autocomplete:"new-password",placeholder:"API key"}),b=Ze("Save key",async()=>{let P=f.value.trim();if(!P)return;b.disabled=!0;let w=await pt(()=>ze.setSecret(v.id,P),"Key saved");b.disabled=!1,w!==void 0&&(f.value="",m(),await s())},{kind:"primary"});f.addEventListener("keydown",P=>{P.key==="Enter"&&b.click()}),m=Ln(`Set key \xB7 ${v.id}`,y("div.stack",null,yt("API key",f),y("div.prov-row",null,y("span.prov-spacer"),b)))},h=v=>{let m=()=>{},f=Ze("Remove provider",async()=>{await pt(()=>ze.removeProvider(v.id),`Removed ${v.id}`)!==void 0&&(m(),await s())},{kind:"danger"}),b=Ze("Cancel",()=>m(),{});m=Ln(`Remove ${v.id}?`,y("div.stack",null,y("div.prov-row",null,y("span.prov-spacer"),b,f)))},d=(v,m,f,b)=>{let P=()=>{},w=jt({value:m.id,placeholder:"provider id",autocomplete:"off",disabled:!m.idEditable}),T=jt({value:m.base_url,placeholder:"https://",autocomplete:"off"}),E=typeof m.auth=="object"&&"env"in m.auth?m.auth.env:null,D=E===null?null:jt({value:E,placeholder:"ENV_VAR_NAME",autocomplete:"off"}),g=m.models===null?null:jt({value:m.models.join(", "),autocomplete:"off"}),S=m.keyField?jt({type:"password",autocomplete:"new-password",placeholder:"Optional"}):null,_=[yt("Provider id",w),yt("Base URL",T)];D&&_.push(yt("Auth env var",D)),g&&_.push(yt("Models",g,"Comma-separated")),S&&_.push(yt("API key",S,"Optional"));let A=Ze(f,async()=>{let I=w.value.trim();if(!I){sn("Give the provider an id","error");return}A.disabled=!0;let k=await b({id:I,base_url:T.value.trim()||null,auth:D?{env:D.value.trim()}:m.auth,models:g?Qy(g.value):null,key:S?.value.trim()??""});A.disabled=!1,k&&P()},{kind:"primary"});P=Ln(v,y("div.stack",null,m.notes?y("p.faint",null,m.notes):null,..._,y("div.prov-row",null,y("span.prov-spacer"),A)))},u=v=>{let m=Qf(v,e);d(`Edit ${v.id}`,{id:v.id,base_url:v.base_url??"",auth:v.auth,models:v.models,idEditable:!1,keyField:!1,notes:m?.notes},"Save",async f=>await pt(()=>ze.updateProvider({id:v.id,kind:v.kind,base_url:f.base_url,auth:f.auth,models:f.models??v.models,options:v.options??void 0,extra_headers:v.extra_headers}),"Saved")===void 0?!1:(await s(),!0))},p=v=>{d(v.label,{id:v.id,base_url:v.base_url??"",auth:v.auth,models:null,idEditable:!0,keyField:!0,notes:v.notes},"Add provider",async m=>await pt(()=>ze.addProvider({id:m.id,kind:v.kind,base_url:m.base_url,auth:m.auth,models:v.models,options:v.options}),`Added ${m.id}`)===void 0?!1:(m.key&&await pt(()=>ze.setSecret(m.id,m.key),"Key saved"),await s(),!0))},x=()=>{let v=()=>{},m=new Set(t.map(E=>E.id)),f=e.filter(E=>!m.has(E.id)),b=jt({type:"search",placeholder:"Search providers",autocomplete:"off"}),P=y("div.prov-kind-grid"),w=E=>{v(),p(E)},T=()=>{let E=b.value.trim().toLowerCase(),g=f.filter(S=>!E||`${S.label} ${S.id} ${S.kind}`.toLowerCase().includes(E)).map(S=>{let _=y("button.prov-kind",{type:"button"},Jl(S.brand,!0),y("span.truncate",null,S.label));return _.addEventListener("click",()=>w(S)),_});Be(P,...g.length?g:[y("p.faint",null,f.length?"No matches":"Every provider is already configured")])};b.addEventListener("input",T),v=Ln("Add provider",y("div.stack",null,b,P)),T()};return s(),()=>{}}function tp(n,e){let t=e[0]==="policy"?"policy":"providers",i=y("div.stack");return n.append(y("div.view-head",null,y("h1",null,"Models")),ju([{key:"providers",label:"Providers"},{key:"policy",label:"Policy & defaults"}],t,r=>Mn("models",...r==="policy"?["policy"]:[])),i),t==="policy"?jf(i):ep(i)}var e_=40;function np(n){let e=y("div.grid.cols-4"),t=y("div"),i=y("ol.feed"),r=y("div");n.append(y("div.view-head",null,y("h1",null,"Overview")),e,y("div.grid.cols-2.overview-main",null,t,Xt("Activity",i,{flush:!0})),Xt("Fleet",r,{flush:!0}));let s=[cr.watch(o=>{if(!o)return Be(e,ft("alert","apid is unreachable","Start apid, then reload."));let a=(o.tasks.open??0)+(o.tasks.claimed??0);Be(e,ni("Agents",o.agents.total,`${o.agents.working} working \xB7 ${o.agents.offline} offline`),ni("Open tasks",a,`${o.tasks.done??0} done`),ni("Worker model",o.default_model.split("/").pop()??"\u2014",o.default_model.split("/")[0]),ni("Orchestrator model",o.orchestrator_model.split("/").pop()??"\u2014",o.orchestrator_model.split("/")[0]))}),dt.watch(o=>{t_(t,o.find(a=>a.tier==="orchestrator")),n_(r,o)}),Mt("transcript",o=>i_(i,o))];return Be(i,y("li.feed-empty.faint",null,"Waiting for activity\u2026")),()=>s.forEach(o=>o())}function t_(n,e){if(!e){Be(n,Xt("Orchestrator",ft("orchestrator","No orchestrator connected","Add the orchestrator role to the fleet to delegate work by goal.")));return}let t=ii({placeholder:"Tell the orchestrator what you want done\u2026",rows:3}),i=Ze("Send",()=>r(),{kind:"primary",icon:"send"}),r=async()=>{let o=t.value.trim();if(!o)return;i.disabled=!0;let a=await pt(()=>ze.chat(e.cid,o),"Sent to the orchestrator");i.disabled=!1,a&&(t.value="")};t.addEventListener("keydown",o=>{o.key==="Enter"&&(o.metaKey||o.ctrlKey)&&r()});let s=e.status==="working"?at("Working","info",!0):e.status==="idle"?at("Idle","ok",!0):at("Offline","muted",!0);Be(n,Xt(y("div.row",null,pi("orchestrator"),y("h2",null,ln(e)),y("span.faint",null,xn(e.role)),s),y("div.stack",null,y("p.muted",null,`Plans the work and delegates it to the fleet \xB7 ${e.model}`),t,y("div.row",null,y("span.faint",null,"Ctrl+Enter to send"),y("span",{style:"flex:1"}),y("a",{href:bn("agents",e.cid)},"Open transcript"),i))))}function n_(n,e){if(!e.length)return Be(n,ft("agents","No agents yet","Agents appear here as their VMs connect."));Be(n,y("div.fleet",null,e.map(t=>{let i=t.status==="working"?"info":t.status==="idle"?"ok":"muted",r=y("button.fleet-tile",{type:"button",title:`${ln(t)} \xB7 ${xn(t.role)} \xB7 ${t.model}`},y("span.row",null,y("span.dot",{class:i}),y("strong.truncate",null,ln(t))),y("span.faint.truncate",null,t.work?Qt(t.work.prompt,60):t.status==="offline"?"offline":xn(t.role)));return r.addEventListener("click",()=>Mn("agents",t.cid)),r})))}function i_(n,e){let t=r_(e);if(!t)return;n.querySelector(".feed-empty")?.remove();let i=dt.get().find(o=>o.cid===e.cid),r=i?ln(i):`agent ${e.cid}`,s=y("li",null,y("span.feed-time.faint",null,qt(e.at_ms)),y("a",{href:bn("agents",e.cid)},r),y("span.muted",null,t));for(n.prepend(s);n.children.length>e_;)n.lastElementChild?.remove()}function r_(n){let e=n.data;switch(n.kind){case"work_start":return`started ${String(e.kind)}: ${Qt(String(e.prompt??""),80)}`;case"work_end":return`finished (${String(e.status)})`;case"tool_request":return`called ${String(e.name)}`;case"error":return`error: ${Qt(String(e.message??""),80)}`;default:return null}}function ip(n){return n.append(y("div.view-head",null,y("h1",null,"Society"))),()=>{}}var s_=[{label:"Open",idle:"Nothing open."},{label:"In progress",idle:"Nothing in progress."},{label:"Done",idle:"Nothing finished yet."},{label:"Failed or withdrawn",idle:"Nothing failed or withdrawn."}],o_=new Set(["failed","withdrawn","cancelled"]),a_=new Set(["work_start","tool_request","work_end","error"]);function Bl(n){return n==="open"?0:n==="done"?2:o_.has(n)?3:1}function l_(n){switch(Bl(n)){case 0:return"info";case 1:return"accent";case 2:return"ok";default:return n==="withdrawn"?"muted":"danger"}}var ws=n=>typeof n=="string"||typeof n=="number"?String(n):"",rp=n=>n.split(`
`).find(e=>e.trim())?.trim()??"Untitled task",Fu=n=>n.replace(/_/g," ").replace(/^./,e=>e.toUpperCase()),c_=n=>{let e=dt.get().find(t=>t.cid===n);return e?ln(e):`agent ${n}`},sp=()=>dt.get().filter(n=>n.status==="idle"&&n.tier==="worker"),u_=n=>dt.get().find(e=>e.cid===n)?.status??"offline",Hl=n=>String(n).padStart(2,"0"),op=n=>{let e=Math.max(0,Math.floor(n/1e3));return`${Hl(Math.floor(e/60))}:${Hl(e%60)}`},h_=n=>{let e=Math.max(0,Math.round(n/1e3));return e<60?`${e}s`:e<3600?`${Math.floor(e/60)}m ${Hl(e%60)}s`:`${Math.floor(e/3600)}h ${Hl(Math.floor(e%3600/60))}m`},d_=n=>new Date(n).toLocaleTimeString([],{hour:"2-digit",minute:"2-digit",second:"2-digit"}),Ou=n=>ws(n.data.tool)||ws(n.data.name)||"tool";function lp(n){let e=n.data.input??{};return Ou(n).toLowerCase()==="bash"&&typeof e.command=="string"?e.command:null}function f_(n){switch(n.kind){case"tool_request":{let e=lp(n);return e!==null?`$ ${e}`:`\u2192 ${Ou(n)}`}case"model_request":return"thinking\u2026";case"work_start":return"starting\u2026";case"work_end":return"wrapping up\u2026";default:return null}}function p_(n){switch(n.kind){case"work_start":return`Started \xB7 ${ws(n.data.kind)||"work"}`;case"tool_request":{let e=lp(n);return e!==null?`$ ${e}`:`\u2192 ${Ou(n)}`}case"work_end":{let e=ws(n.data.status);return e?`Finished \xB7 ${e}`:"Finished"}default:return ws(n.data.message)||ws(n.data.error)||"Error"}}function m_(n,e){let t=new Set(e.filter(i=>i.kind==="work_start"&&((i.data.origin??{}).task===n||i.data.task===n)).map(i=>i.work));return e.filter(i=>t.has(i.work)&&a_.has(i.kind))}function ap(n){if(n===null)return y("span.faint",null,"Unassigned");let e=y("a",{href:bn("agents",n)},c_(n));return e.addEventListener("click",t=>t.stopPropagation()),e}var g_=n=>y("li",{class:`k-${n.kind}`},y("time",null,d_(n.at_ms)),y("span",null,p_(n)));function cp(n,e){let t=e[0]??"",i=[],r=null,s=null,o=!1,a,l=0,c,h=new Map,d=new Map,u=new Set,p=[],x=()=>{Be(n,t?D():E())},v=async _=>{if(s=null,_.agent_cid===null){s=[];return}try{s=m_(_.id,await ze.transcript(_.agent_cid))}catch{s=[]}},m=async()=>{let _=++l;try{if(t){let A=await ze.task(t);if(_!==l||(r=A,o=!1,await v(A),_!==l))return}else{let A=await ze.tasks();_===l&&(i=A)}}catch{_===l&&t&&(r=null,o=!0)}_===l&&x()},f=()=>{clearTimeout(a),a=setTimeout(()=>{m()},300)},b=()=>{let _=jt({required:!0,placeholder:"What should get done?"}),A=ii({rows:4}),I=ii({rows:3}),k=Array.from(new Set(dt.get().filter(ie=>ie.tier==="worker").map(ie=>ie.role).filter(Boolean))).sort(),N=gn([{value:"",label:"Any"},...k.map(ie=>({value:ie,label:ie}))]),U=jt({type:"number",min:0,step:1,value:100}),z=gn([{value:"",label:"Let the scheduler choose"},...sp().map(ie=>({value:String(ie.cid),label:ln(ie)}))]),Y=y("button.btn.primary",{type:"submit"},pi("plus"),"Create task"),ee=y("form.stack",null,yt("Title",_),yt("Details",A),yt("Acceptance",I),y("div.grid.cols-2",null,yt("Role",N),yt("Budget",U)),yt("Assign to",z),y("div.row",null,y("span",{style:"flex:1"}),Y));ee.addEventListener("submit",async ie=>{ie.preventDefault();let ce=_.value.trim();if(!ce){_.focus();return}let Se=A.value.trim();Y.disabled=!0;let fe=await pt(()=>ze.createTask({goal:Se?`${ce}

${Se}`:ce,acceptance:I.value.trim()||void 0,role:N.value||void 0,budget:U.value===""?100:Number(U.value),assignee:z.value?Number(z.value):void 0}),"Task created");Y.disabled=!1,fe&&(J(),Mn("tasks",fe.id))});let J=Ln("New task",ee,()=>{c=void 0});c=J};function P(_){let A=N=>_.filter(U=>Bl(U.state)===N).length,I=dt.get().filter(N=>N.status==="working").length,k=(N,U)=>y("div.tk-fig",null,y("b",null,String(U)),y("span",null,N));return y("div.tk-summary",null,k("Open",A(0)),k("In progress",A(1)),k("Done",A(2)),k("Failed",A(3)),k("Agents working",I))}function w(_){let A=Bl(_.state),I=d.get(_.id),k=["task-card",`lane-${A}`];u.has(_.id)||k.push("enter"),I!==void 0&&I!==_.state&&k.push("changed"),u.add(_.id),d.set(_.id,_.state);let N=_.poster,U=y("div.task-who",null,y("span.task-dot",{class:`s-${u_(_.agent_cid)}`}),ap(_.agent_cid),N?y("span.faint",null,`by ${N}`):null),z;if(A===1){let J=y("span.task-elapsed",null,op(Date.now()-_.updated_at));p.push({el:J,since:_.updated_at});let ie=_.agent_cid!==null&&h.get(_.agent_cid)||"waiting for the first step\u2026";z=y("div.task-live",null,y("div.task-stripe"),y("div.task-activity",{"data-activity":String(_.agent_cid??"")},ie),y("div.task-meta",null,y("span.faint",null,"elapsed"),J))}else A===2?z=y("div",null,_.note?y("p.task-note",null,Qt(_.note,120)):null,y("div.task-meta",null,y("span.faint",null,`took ${h_(_.updated_at-_.created_at)}`),y("span.faint",null,qt(_.updated_at)))):A===3?z=y("div",null,y("p.task-reason",null,_.outcome||_.note||Fu(_.state)),y("div.task-meta",null,y("span.faint",null,Fu(_.state)),y("span.faint",null,qt(_.updated_at)))):z=y("div.task-meta",null,y("span.faint",null,`budget ${_.budget}`),y("span.faint",null,`created ${qt(_.created_at)}`));let Y=y("div",{class:k.join(" "),role:"link",tabindex:0},y("div.task-top",null,y("span.task-id",null,_.id),y("span.task-role",null,_.role?xn(_.role):"Any role")),y("strong.task-title",null,rp(_.goal)),U,z),ee=()=>Mn("tasks",_.id);return Y.addEventListener("click",ee),Y.addEventListener("keydown",J=>{J.key==="Enter"&&ee()}),Y}function T(_){p=[];let A=_.slice().sort((I,k)=>k.updated_at-I.updated_at);return y("div.tk-board-wrap",null,y("div.tk-board",null,s_.map((I,k)=>{let N=A.filter(U=>Bl(U.state)===k);return y("section.tk-lane",{class:`lane-${k}`},y("header.tk-lane-head",null,y("span.tk-mark",{class:`mark-${k}`}),y("h2",null,I.label),y("span.tk-count",null,String(N.length))),y("div.tk-lane-body",null,N.length?N.map(w):y("p.tk-empty",null,I.idle)))})))}function E(){return y("div.tasks-view",null,y("div.view-head",null,y("h1",null,"Tasks"),y("div.actions",null,Ze("New task",b,{kind:"primary",icon:"plus"}))),P(i),T(i))}function D(){let _=y("a.back",{href:bn("tasks")},pi("chevronRight"),"Tasks");if(o)return y("div.tasks-view",null,_,y("p.tk-empty",null,"Task not found."));let A=r;if(!A)return y("div.tasks-view",null,_);let I=A.state==="open",k=gn([{value:"",label:"Assign to\u2026"},...sp().map(ie=>({value:String(ie.cid),label:ln(ie)}))],"");k.addEventListener("change",async()=>{k.value&&(k.disabled=!0,await pt(()=>ze.assign(Number(k.value),A.id),"Assigned"),k.disabled=!1,m())});let N=Ze("Withdraw",async()=>{N.disabled=!0,await pt(()=>ze.withdrawTask(A.id),"Task withdrawn")?m():N.disabled=!1},{kind:"danger",icon:"x"}),U=y("dl.kv",null,y("dt",null,"State"),y("dd",null,at(Fu(A.state),l_(A.state),!0)),y("dt",null,"Role"),y("dd",null,A.role?xn(A.role):"Any role"),y("dt",null,"Assignee"),y("dd",null,ap(A.agent_cid)),y("dt",null,"Budget"),y("dd",null,String(A.budget)),y("dt",null,"Created"),y("dd",null,qt(A.created_at)),y("dt",null,"Updated"),y("dd",null,qt(A.updated_at))),z=s,Y=z===null?y("p.tk-empty",null,"Loading\u2026"):z.length===0?y("p.tk-empty",null,A.agent_cid===null?"Not yet assigned.":"No steps recorded yet."):y("ol.tk-tl",null,z.map(g_)),ee=y("div.tk-main",null,y("section.tk-block",null,y("h3.tk-label",null,"Goal"),y("div.tk-goal",null,A.goal)),A.acceptance?y("section.tk-block",null,y("h3.tk-label",null,"Acceptance"),y("div.tk-prose",null,A.acceptance)):null,A.note?y("section.tk-block",null,y("h3.tk-label",null,"Result"),y("div.tk-result",null,A.note)):null),J=y("aside.tk-side",null,y("section.tk-block",null,y("h3.tk-label",null,"Facts"),U,I?k:null,I?y("div.row",null,y("span",{style:"flex:1"}),N):null),y("section.tk-block",null,y("h3.tk-label",null,"Timeline"),Y));return y("div.tasks-view",null,_,y("div.view-head",null,y("div.tk-head",null,y("span.task-id",null,A.id),y("h1",null,rp(A.goal)))),y("div.tk-cols",null,ee,J))}x();let g=setInterval(()=>{let _=Date.now();for(let A of p)A.el.textContent=op(_-A.since)},1e3),S=[Mt("task",f),Mt("work",f),Mt("transcript",_=>{let A=_;if(t){r&&A.cid===r.agent_cid&&f();return}let I=f_(A);I!==null&&(h.set(A.cid,I),n.querySelectorAll(`[data-activity="${A.cid}"]`).forEach(k=>{k.textContent=I}))}),dt.watch(()=>x())];return m(),()=>{clearTimeout(a),clearInterval(g),S.forEach(_=>_()),p=[],c?.()}}var Si=n=>n.toLocaleString("en-US"),Bu=n=>n instanceof Error?n.message:String(n),x_={mint:"ok",spend:"warn",payment:"accent",refund:"info",transfer:"info",fine:"danger"},up=n=>n.postings.reduce((e,t)=>t.amount>0?e+t.amount:e,0),v_=(n,e)=>n.filter(t=>t.kind===e).reduce((t,i)=>t+up(i),0),y_=(n,e,t,i,r)=>{let s=(c,h,d,u)=>`<rect class="tr-box" x="${c}" y="${h}" width="170" height="70"/><text class="tr-label" x="${c+12}" y="${h+22}">${d}</text><text class="tr-fig" x="${c+12}" y="${h+54}">${Si(u)}</text>`,o=(c,h,d,u)=>`<line class="tr-line" x1="${c}" y1="${h}" x2="${d}" y2="${u}" marker-end="url(#tr-arrow)"/>`,a='<svg class="tr-flow" viewBox="0 0 830 180" role="img" aria-label="Where the plaza budget goes"><defs><marker id="tr-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path class="tr-head" d="M0 0L10 5L0 10z"/></marker></defs>'+s(0,55,"Budget",n)+s(220,55,"Escrow",e)+s(440,10,"Spend",t)+s(440,100,"Payment",i)+s(660,55,"Wallets",r)+o(170,90,220,90)+o(390,90,440,45)+o(390,90,440,135)+o(610,135,660,90)+"</svg>",l=y("div.tr-flow-wrap");return l.innerHTML=a,l};function hp(n,e){let t=y("div.stack"),i=!0,r,s="",o=null,a=()=>{let v=jt({type:"number",min:1,step:1,required:!0}),m=()=>{},f=Ze("Mint",()=>{let b=Number(v.value);if(!Number.isInteger(b)||b<=0){sn("Enter a whole amount above zero.","error");return}f.disabled=!0,ze.mintBudget(b).then(()=>{sn(`Minted ${Si(b)} into the plaza budget.`),m(),u()}).catch(P=>{f.disabled=!1,sn(Bu(P),"error")})});f.classList.add("primary"),m=Ln("Mint budget",y("div.stack",null,yt("Amount",v,"Whole units added to the plaza budget."),y("div.row",null,f)))},l=()=>{let v=o?.roster??[];if(v.length===0){sn("There are no agents to fine.","error");return}let m=gn(v.map(T=>({value:String(T.cid),label:T.name}))),f=jt({type:"number",min:1,step:1,required:!0}),b=jt({type:"text",required:!0,placeholder:"What for"}),P=()=>{},w=Ze("Fine",()=>{let T=Number(f.value),E=b.value.trim();if(!Number.isInteger(T)||T<=0){sn("Enter a whole amount above zero.","error");return}if(!E){sn("A fine needs a memo.","error");return}w.disabled=!0,ze.fineAgent(Number(m.value),T,E).then(()=>{sn(`Fined ${Si(T)}.`),P(),u()}).catch(D=>{w.disabled=!1,sn(Bu(D),"error")})});w.classList.add("danger"),P=Ln("Fine an agent",y("div.stack",null,yt("Agent",m),yt("Amount",f,"Taken from the agent\u2019s wallet."),yt("Memo",b,"Shown in the journal."),y("div.row",null,w)))},c=Ze("Fine an agent",()=>l()),h=Ze("Mint budget",()=>a());h.classList.add("primary"),n.append(y("div.view-head",null,y("h1",null,"Treasury"),y("div.actions",null,c,h)),t);let d=()=>{if(!o)return;let{t:v,journal:m,wallets:f,roster:b}=o,P=new Map(b.map(k=>[k.cid,k.role_label])),w=v.accounts.filter(k=>k.kind==="escrow").reduce((k,N)=>k+N.balance,0),T=v_(m,"payment"),E=f.reduce((k,N)=>k+N.balance,0),D=[...new Set(m.map(k=>k.kind))].sort(),g=m.filter(k=>!s||k.kind===s).slice(0,50),S=[{label:"Agent",cell:k=>y("a",{href:`#/agents/${k.cid}`},k.name)},{label:"Role",cell:k=>P.get(k.cid)??"\u2014"},{label:"Balance",num:!0,cell:k=>Si(k.balance)}],_=f.slice().sort((k,N)=>N.balance-k.balance),A=gn([{value:"",label:"All kinds"},...D.map(k=>({value:k,label:k}))],s);A.addEventListener("change",()=>{s=A.value,d()});let I=g.map(k=>y("div.tr-row",null,y("span.tr-n",null,`#${k.id}`),y("span.tr-at",null,qt(k.at)),at(k.kind,x_[k.kind]??"muted"),y("span.tr-memo",null,k.memo||`${k.kind}${k.task?` for ${k.task}`:""}`),y("span.tr-amt",null,Si(up(k)))));Be(t,y("div.grid.tr-tallies",null,ni("Plaza budget",Si(v.plaza_budget)),ni("In escrow",Si(w),"held for open tasks"),ni("Spent",Si(v.spent),"to providers and compute"),ni("Paid to agents",Si(T),"margins for verified work"),ni("Agents\u2019 wallets",Si(E))),Xt("Flow",y_(v.plaza_budget,w,v.spent,T,E)),Xt("Wallets",f.length?ur(S,_):ft("user","No wallets yet."),{flush:!0}),Xt("Journal",g.length?y("div.tr-ledger",null,...I):ft("clock","No entries."),{flush:!0,actions:[A]}))},u=async()=>{try{let[v,m,f,b]=await Promise.all([ze.treasury(),ze.treasuryJournal(),ze.treasuryWallets(),ze.agents()]);if(!i)return;o={t:v,journal:m.slice().sort((P,w)=>w.at-P.at),wallets:f,roster:b},d()}catch(v){i&&Be(t,ft("alert","Treasury unreachable",Bu(v)))}},x=Mt("treasury",()=>{r!==void 0&&clearTimeout(r),r=setTimeout(()=>{r=void 0,u()},400)});return u(),()=>{i=!1,x(),r!==void 0&&clearTimeout(r)}}var zl=[{key:"overview",label:"Overview",icon:"overview",section:"Plaza",mount:np},{key:"agents",label:"Agents",icon:"agents",section:"Plaza",mount:Nf},{key:"tasks",label:"Tasks",icon:"tasks",section:"Plaza",mount:cp},{key:"floor",label:"Floor",icon:"floor",section:"Plaza",mount:Af},{key:"society",label:"Society",icon:"society",section:"Social",mount:ip},{key:"conversations",label:"Conversations",icon:"conversations",section:"Social",mount:Of},{key:"board",label:"Board",icon:"board",section:"Social",mount:Ff},{key:"council",label:"Council",icon:"council",section:"Social",mount:zf},{key:"treasury",label:"Treasury",icon:"treasury",section:"Social",mount:hp},{key:"models",label:"Models",icon:"models",section:"Setup",mount:tp},{key:"machine",label:"Machines",icon:"machine",section:"Setup",mount:Kf}];var dp=new Map,fp=new Map;function __(){let n=y("nav.nav",{"aria-label":"Screens"}),e="";zl.forEach((r,s)=>{r.section!==e&&(e=r.section,n.appendChild(y("div.nav-section",null,e)));let o=y("span.count");fp.set(r.key,o);let a=y("a",{href:bn(r.key)},y("span.k",null,String(s+1)),r.label,y("span.dots"),o);dp.set(r.key,a),n.appendChild(a)});let t=y("div.sidebar-foot");qu(r=>Be(t,y("span.row",null,mi(r?"ok":"danger"),r?"Live":"Reconnecting\u2026")));let i=y("div.brand",null,y("span.brand-mark"),"AgentPlaza",y("small",null,"the plaza register"));return y("aside.sidebar",null,i,n,t)}function b_(n){let e=y("div.health");return cr.watch(t=>{if(!t)return Be(e,y("span",null,mi("danger"),"apid unreachable"));Be(e,y("span",null,mi(t.hub?"ok":"danger"),"Hub"),y("span",null,mi(t.noded?"ok":"danger"),"Node"),y("span",null,mi(t.agents.working?"info":"muted"),`${t.agents.working} working \xB7 ${t.agents.idle} idle`))}),y("header.topbar",null,n,y("span.spacer"),e)}function M_(){let n=y("div",{hidden:!0});return cr.watch(e=>{n.hidden=!e?.dev_models,Be(n,pi("alert"),"Development models are enabled: the model policy is relaxed (mock and local models allowed)."),n.className="banner danger"}),n}function S_(){$u();let n=y("span.title"),e=y("main.view-host"),t=y("div.app",null,__(),y("div.main",null,b_(n),M_(),e));document.body.replaceChildren(t),dt.watch(s=>{let o=s.filter(a=>a.status==="working").length;fp.get("agents").textContent=s.length?o?`${o}/${s.length}`:String(s.length):""});let i,r="";Gu(s=>{let o=zl.find(c=>c.key===s.screen)??zl[0];for(let[c,h]of dp)c===o.key?h.setAttribute("aria-current","page"):h.removeAttribute("aria-current");let a=`${o.key}/${s.params.join("/")}`;if(a===r)return;r=a,i?.(),n.textContent=o.label,document.title=`${o.label} \xB7 AgentPlaza`;let l=y("div.view");e.replaceChildren(l),i=o.mount(l,s.params)})}S_();
/*! Bundled license information:

three/build/three.core.js:
three/build/three.module.js:
  (**
   * @license
   * Copyright 2010-2026 Three.js Authors
   * SPDX-License-Identifier: MIT
   *)
*/
