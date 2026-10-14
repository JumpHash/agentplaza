# GPU reference designs for the hardware view

Status: research pass, 2026-10-13; the data is built (2026-10-14). The cards are `assets/hardware/gpus/specs.json` and
`match.json`, read by `crates/apz-engine/src/gpus.rs` (a port of the retired `apps/web/src/views/floor3d/gpus.ts`), and
the hardware view matches a detected GPU to a card through them (`engine-port.md`, row 6). The units conflict in section 0 is
still written as open in this doc. Scope: NVIDIA, AMD and Intel product segments (consumer, workstation, data centre and server, edge and embedded, laptop, integrated), plus Apple Silicon as integrated. Nothing was downloaded. Values come from manufacturer spec pages and reviews read as search excerpts or short page reads.

Conventions:
- `~` marks an approximate or unconfirmed value.
- "unverified" means no source read in this pass confirmed it. Those fields are for the modeller to decide, using the generic fallback.
- Hex colours are estimates unless stated. None were sampled from photographs.
- "Ref" means a manufacturer reference design exists. "No ref" means partner-only, so use the generic form.
- Each card id is the key a machine model or detection rule should use. The matching table and fallbacks are in section 5.

## 0. Scale and AgentPlaza kinds

- Owner brief: about 1 unit = 10 cm for GPU modelling. A 304 mm card is about 3.0 units long, 1.4 wide and 0.4 to 0.6 thick.
- Conflict to resolve before modelling: `docs/design/hardware-plazas.md` and `assets/hardware/README.md` define one unit as about one robot's width, and a robot's width is not 10 cm. A GPU part built at 10 cm would be out of scale in those views. Pick one convention and document it. This doc uses the owner's 10 cm figure for the dimensions below.
- AgentPlaza kinds in use: `workstation`, `laptop`, `edge`, `phone`, `server`.
  - Cards for desktops: `workstation`.
  - Data-centre cards, OAM/SXM modules, HGX and DGX chassis: `server`.
  - Jetson, IGX, Versal/Kria boards and the NUC-class box: `edge`.
  - Laptop dGPU boards: `laptop`.
  - DGX Spark and DGX Station are desktop-class, so `workstation`.
  - `phone`: no discrete GPU in scope; the phone-class parts in this doc are none.
- Apple Silicon is already modelled: `exterior/apple-laptop.json`, `exterior/apple-desktop.json` and `board/apple-soc.json` cover it. No new card model is needed for it.

## 1. Reference status at a glance

Partner-only (no manufacturer reference board). Use the generic form in section 5.
- NVIDIA: GeForce RTX 5070 Ti, RTX 4070 Ti, RTX 4070 Ti SUPER.
- AMD: Radeon RX 9070 XT (AMD published no reference board; its own render was marked "not available for purchase").
- Intel: Arc B570 (per Club386, partner-only), Arc Pro B60 (Intel publishes only an I/O layout; boards come from partners such as Sparkle and ASRock), Arc Pro A60 (builder not confirmed).

Reference exists but is sold only through OEM or partner systems: H100 NVL, H200 NVL, DGX Station GB300, IGX Orin, Gaudi 3 HLB-325 baseboard (Dell, Supermicro and Gigabyte build around it), Jetson Orin NX Super carrier (module is NVIDIA's, carriers are partner-built).

Cards the owner did not list, which are worth adding. Recalled, not researched in this pass; verify before modelling:
- NVIDIA GeForce RTX 3080 FE and RTX 3070 FE (the Founders design language starts here).
- AMD Radeon RX 6800 XT reference (MBA).
- AMD Radeon RX 7800 XT and RX 7700 XT: believed partner-only.
- Intel Arc B570: partner-only, from Club386 (section 4.1).

## 2. NVIDIA

### 2.1 Consumer

Generic partner fallback: `nvidia-consumer-generic`. About 2.5 to 3 slots, about 290 to 300 mm, triple-fan open-air, black or silver shroud (estimate).

**nvidia-rtx5090-fe** — GeForce RTX 5090 Founders Edition
- Form: PCIe, 2-slot per NVIDIA (press says ~2.5; NVIDIA's case-clearance note says 3). 304 x 137 x 40 mm. Full-length, full-height.
- Cooling: two axial fans in dual flow-through. NVIDIA says vapour chamber with heat pipes and active fins. Fan diameter unverified.
- Shroud/back: graphite shroud, ~#2A2A2A (estimate). Backplate unverified.
- Brand: "GEFORCE RTX" text and light position unverified.
- Power: one 12V-2x6 (16-pin). Position unverified.
- I/O: 3x DisplayPort, 1x HDMI. Bracket ~dark grey.
- Kind: workstation. Ref: yes (NVIDIA FE).
- Src: nvidia.com/en-us/geforce/graphics-cards/50-series/rtx-5090/ ; techpowerup.com/gpu-specs/geforce-rtx-5090.c4216

**nvidia-rtx5080-fe** — GeForce RTX 5080 Founders Edition
- Form: 2-slot, 304 x 137 mm (NVIDIA). Thickness unverified. Full-length.
- Cooling: two fans, same family as the 5090 FE (~, inferred).
- Shroud/back, brand: unverified.
- Power: 12V-2x6 native (~). NVIDIA lists 3x 8-pin via adapter or a 450 W+ PCIe Gen5 cable as supply options.
- I/O: 3x DisplayPort, 1x HDMI.
- Kind: workstation. Ref: yes.
- Src: nvidia.com/en-us/geforce/graphics-cards/50-series/rtx-5080/

**nvidia-rtx5070-fe** — GeForce RTX 5070 Founders Edition
- Form: 242 x 112 mm (NVIDIA family page). Slot count ~2. Full-length.
- Cooling: dual-fan open-air (LANOC review).
- Shroud/back: cast shroud with an "infinity loop" shape on both sides (LANOC). Colour ~graphite grey.
- Brand, power, I/O: unverified.
- Kind: workstation. Ref: yes.
- Src: nvidia.com/en-us/geforce/graphics-cards/50-series/rtx-5070-family/ ; lanoc.org (RTX 5070 FE review)

**nvidia-rtx5070ti-generic** — GeForce RTX 5070 Ti (no FE)
- No ref. Generic: ~2.5-slot, ~300 mm, dual- or triple-fan open-air (estimate).
- Kind: workstation.
- Src: tomshardware.com (Blackwell FE details)

**nvidia-rtx4090-fe** — GeForce RTX 4090 Founders Edition
- Form: 3-slot, 304 x 137 x 61 mm (TechPowerUp, NVIDIA). Full-length.
- Cooling: triple axial fans (Server The Home). The fan count conflicts with other reports for the 4080 FE; confirm before modelling. Fan diameter unverified.
- Shroud/back: X-shaped frame (widely reported; not confirmed in any source read here). Shroud ~#1A1A1A. Backplate unverified.
- Brand: unverified.
- Power: 16-pin 12VHPWR, later 12V-2x6. Position unverified.
- I/O: ~3x DP, 1x HDMI (general knowledge).
- Kind: workstation. Ref: yes.
- Src: techpowerup.com/gpu-specs/geforce-rtx-4090.c3889 ; servethehome.com (RTX 4090 FE review)

**nvidia-rtx4080-fe** — GeForce RTX 4080 Founders Edition (also covers the 4080 SUPER FE, assumed the same board, ~)
- Form: 3-slot, 304 x 137 x 61 mm (NVIDIA quick-start guide). Full-length.
- Cooling: two fans, flow-through (Reddit; conflicts with the 4090's triple fans). Fan diameter unverified.
- Shroud/back, brand, power, I/O: unverified.
- Kind: workstation. Ref: yes (4080 FE and 4080 SUPER FE).
- Src: nvidia.com/content/geforce-gtx/GeForce_RTX_4080_QSG_Rev1.pdf ; nvidia.com/en-us/geforce/graphics-cards/40-series/rtx-4080-family/

**nvidia-rtx4070-fe** — GeForce RTX 4070 Founders Edition
- Form: dual-slot, ~242 x 112 mm (TechPowerUp lists 240 x 110 x 40). Full-length.
- Cooling: ~two fans, flow-through (~, similar chassis to the 3080 FE). Fan count unverified.
- Shroud/back, brand, power, I/O: unverified.
- Kind: workstation. Ref: yes.
- Src: techpowerup.com/gpu-specs/geforce-rtx-4070.c3924

**nvidia-rtx4070ti-generic** — GeForce RTX 4070 Ti and 4070 Ti SUPER (no FE)
- No ref. Generic: ~2.5-slot, ~290 mm, triple-fan open-air (estimate).
- Kind: workstation.
- Src: xda-developers.com (4070 Ti SUPER vs 4070)

**nvidia-rtx3090-fe** — GeForce RTX 3090 Founders Edition
- Form: 3-slot, 313 x 138 mm (NVIDIA). Full-length.
- Cooling: flow-through, two fans (~), ~110 mm fan diameter (teardown snippet).
- Shroud/back, brand, power, I/O: unverified.
- Kind: workstation. Ref: yes.
- Src: nvidia.com/en-us/geforce/graphics-cards/30-series/rtx-3090-3090ti/

### 2.2 Workstation

Generic: `nvidia-workstation-generic` for a dual-slot blower or active card, 111 x 267 mm.

**nvidia-rtxpro6000-ws** — RTX PRO 6000 Blackwell Workstation Edition
- Form: dual-slot, extended height, ~137 x 305 mm (NVIDIA, 5.4 x 12 in). Full-length.
- Cooling: double flow-through with a 3D vapour chamber on both sides and dual heatsinks with fans (Jon Peddie). Two fans per side (~).
- Shroud/back, brand: unverified.
- Power: 600 W board power, 4x auxiliary connectors. Connector sits on the top middle of the card (Jon Peddie).
- I/O: 4x DisplayPort 2.1b.
- Kind: workstation. Ref: yes.
- Src: nvidia.com/en-us/products/workstations/professional-desktop-gpus/rtx-pro-6000-family/ ; jonpeddie.com (RTX PRO 6000 review)

**nvidia-rtxpro6000-maxq** — RTX PRO 6000 Blackwell Max-Q Workstation Edition
- Form: dual-slot, full-height, ~111 x 267 x 40 mm (NVIDIA, TechPowerUp). Full-length.
- Cooling: one blower-style fan with a fully enclosed heatsink (CentralComputer). This is the only single-fan blower in the list.
- Shroud/back: unverified.
- Power: 300 W, one auxiliary cable (PNY). Connector type unverified.
- I/O: 4x DisplayPort 2.1.
- Kind: workstation. Ref: yes.
- Src: nvidia.com/en-us/products/workstations/professional-desktop-gpus/rtx-pro-6000-max-q/ ; techpowerup.com/gpu-specs/rtx-pro-6000-blackwell-max-q-workstation.c4273

**nvidia-rtx6000ada** — RTX 6000 Ada Generation
- Form: dual-slot, full-height, ~111 x 267 mm (NVIDIA). Full-length.
- Cooling: "active" per NVIDIA. Blower assumed (~, not stated on the datasheet).
- Power: one 16-pin (~).
- I/O: 4x DisplayPort 1.4a.
- Kind: workstation. Ref: yes.
- Src: nvidia.com/en-us/products/workstations/rtx-6000/

**nvidia-rtx4000ada** — RTX 4000 Ada Generation (single slot)
- Form: single-slot, 112 x 241 mm (NVIDIA, 4.4 x 9.5 in). Low-profile capable per NVIDIA.
- Cooling: active blower.
- Power: 130 W. Slot-powered or aux connector unverified.
- I/O: 4x DisplayPort 1.4a.
- Brand: unverified.
- Kind: workstation. Ref: yes.
- Src: nvidia.com/en-us/products/workstations/rtx-4000/

**nvidia-rtxa6000** — RTX A6000 (optional, Ampere)
- Form: dual-slot, full-height, ~111 x 267 mm. Full-length.
- Cooling: active; blower assumed (~).
- Power: 1x 8-pin. I/O: 4x DisplayPort 1.4a.
- Kind: workstation. Ref: yes.
- Src: nvidia.com datasheet (RTX A6000)

### 2.3 Laptop dGPU (NVIDIA)

Not researched in this pass. Generic: a square BGA GPU die soldered to the board, with GDDR chips in a ring around it (~). Kind `laptop`. Ref: none; each OEM designs its own board.

### 2.4 Data centre and server, PCIe cards

All cards in this group are passive (no fan) unless noted, and rely on chassis airflow. All have a manufacturer reference product. Kind: `server` for every card here.

**nvidia-h100-pcie** — H100 PCIe
- Form: PCIe Gen5, dual-slot (~40 mm), full-height, full-length. Nominal 267 x 111 mm. Extender options give 312 or 339 mm overall.
- Cooling: passive, bidirectional heatsink that takes airflow from either end. Fin count and heat-pipe layout unpublished.
- Shroud/back: no shroud. Bare finned heatsink, dark grey or black (~). NVLink bridge connectors on the top (north) edge, with a 2.5 mm keep-out above it. Three bridges, 600 GB/s.
- Brand: none documented.
- Power: one PCIe 16-pin (CEM 5) on the east edge. 350 W default.
- I/O: none (no display outputs). Bracket colour not documented.
- Src: nvidia.com/content/dam/en-zz/Solutions/gtcs22/data-center/h100/PB-11133-001_v01.pdf ; techpowerup.com/gpu-specs/h100-pcie-80-gb.c3899

**nvidia-h100-nvl** — H100 NVL (bridged pair)
- Form: PCIe Gen5, dual-slot, full-height, full-length (~267 x 111 mm). Shipped as a bridged pair. Each board spans two slots when bridged.
- Cooling: passive heatsink, front-to-back chassis airflow.
- Shroud/back: no shroud. Finned heatsink, ~dark grey. Three NVLink bridges on the north edge, 188 GB pooled (94 GB per card).
- Power: one PCIe 16-pin on the east edge. Default 400 W, configurable 350 to 400 W.
- I/O: none.
- Kind: server. Ref: yes (NVIDIA reference product; sold mostly in certified systems).
- Src: nvidia.com/content/dam/en-zz/Solutions/Data-Center/h100/PB-11773-001_v01.pdf

**nvidia-h200-nvl** — H200 NVL
- Form: PCIe Gen5, dual-slot, full-height (~267 x 111 mm). Same footprint family as the H100 NVL.
- Cooling: passive heatsink, no fan.
- Shroud/back: no shroud. Finned heatsink, ~dark grey. 2-way or 4-way NVLink bridge on the top edge; a removable cap covers the connector on non-bridged systems. 141 GB HBM3e per card.
- Power: one PCIe 16-pin (12VHPWR class) on the east edge. 600 W maximum by default, with a 350 W compliance limit in some modes.
- I/O: none.
- Kind: server. Ref: yes (sold through MGX and certified systems).
- Src: nvidia.com/en-us/data-center/h200/ ; NVIDIA H200 NVL product spec (PDF)

**nvidia-l40s** — L40S
- Form: PCIe Gen4 x16, dual-slot, full-height, full-length, 111 x 267 mm, ~40 mm thick (TechPowerUp, ~). No NVLink.
- Cooling: passive heatsink, no fan.
- Shroud/back: no shroud. Finned heatsink, ~dark grey.
- Power: one PCIe 16-pin on the east edge (~). 350 W maximum.
- I/O: 4x DisplayPort 1.4a. Standard bracket, colour not documented.
- Kind: server. Ref: yes (sold through partners and OEMs).
- Src: nvidia.com/en-us/data-center/l40s/

**nvidia-l40** — L40
- Form: PCIe Gen4 x16, dual-slot, full-height, 111 x 267 mm. 300 W. No NVLink.
- Cooling: passive heatsink.
- Shroud/back: no shroud. Finned heatsink, ~dark grey.
- Power: one PCIe 16-pin, 300 W.
- I/O: 4x DisplayPort 1.4a.
- Kind: server. Ref: yes (datasheet, Jan 2023 figures are preliminary).
- Src: nvidia.com/en-us/data-center/l40/

**nvidia-l4** — L4
- Form: PCIe Gen4 x16, single-slot, half-height, half-length (HHHL), ~168.5 mm long, ~69 mm tall with the low-profile bracket (~, TechPowerUp gives 56 mm board height). Ships with a low-profile and a full-height bracket. Thickness ~18 to 20 mm (~).
- Cooling: passive, low-profile finned heatsink.
- Shroud/back: no shroud. ~dark grey.
- Power: none; slot-powered, 72 W max, 40 W min.
- I/O: none. Bracket colour not documented.
- Kind: server. Ref: yes. OEM brackets vary (e.g. Lenovo ThinkSystem).
- Src: nvidia.com PB-11316-001 (L4) ; techpowerup.com/gpu-specs/l4.c4091

**nvidia-a100-pcie** — A100 PCIe
- Form: PCIe Gen4 x16, dual-slot, full-height, full-length. Nominal 267 x 111 mm. Card plus extender 312 or 339 mm.
- Cooling: passive heatsink.
- Shroud/back: no shroud. Finned heatsink, ~dark grey. Three NVLink bridges on the north edge (80 GB variant); the 40 GB variant has one bridge (2-card pair).
- Power: one CPU 8-pin (EPS-style) on the east edge. 250 W (40 GB) or 300 W (80 GB).
- I/O: none.
- Kind: server. Ref: yes (40 GB and 80 GB product briefs).
- Src: nvidia.com PB-10577-001 (A100) product brief

**nvidia-a40** — A40
- Form: PCIe Gen4, dual-slot, full-height, 111 x 267 mm. 2-way NVLink bridge.
- Cooling: passive ("passively cooled" per product brief).
- Shroud/back: no shroud. Finned heatsink, ~dark grey. NVLink bridge on the top edge (~).
- Power: one CPU 8-pin. 300 W maximum.
- I/O: display outputs unverified. Bracket colour not documented.
- Kind: server. Ref: yes.
- Src: nvidia.com/en-us/data-center/a40/

**nvidia-a10** — A10
- Form: PCIe Gen4, single-slot, full-height (FHFL), 111 x 267 mm. Single-slot, not dual.
- Cooling: passive, bidirectional airflow per product brief.
- Shroud/back: no shroud. Thin finned heatsink, ~dark grey. No NVLink.
- Power: one PCIe 8-pin on the east edge. 150 W.
- I/O: display outputs unverified (none assumed).
- Kind: server. Ref: yes.
- Src: nvidia.com PB-10415-001 (A10) product brief

**nvidia-a2** — A2
- Form: PCIe Gen4, single-slot, low-profile (HHHL), ~168 x 69 mm, ~20 mm thick (~, TechPowerUp).
- Cooling: passive.
- Shroud/back: no shroud. Low-profile finned heatsink, ~dark grey.
- Power: slot-powered, 60 W (~).
- I/O: none (~). Low-profile bracket.
- Kind: server. Ref: yes.
- Src: nvidia.com A2 product brief ; techpowerup.com/gpu-specs/a2.c3848

**nvidia-t4** — T4
- Form: PCIe Gen3 x16, single-slot, low-profile (HHHL), ~168 x 69 mm.
- Cooling: passive.
- Shroud/back: no shroud. Low-profile finned heatsink, ~dark grey.
- Power: slot-powered, 70 W max (~, no aux connector).
- I/O: none (~). Low-profile bracket.
- Kind: server. Ref: yes.
- Src: nvidia.com/en-us/data-center/tesla-t4/

### 2.5 Data centre and server, modules and systems

**nvidia-sxm5-module** — SXM5 module (H100 and H200 SXM)
- Form: GPU module in an SXM5 socket on the HGX baseboard, not a card. Footprint ~102 x 165 mm (~, third-party PCB comparison, not NVIDIA). TechPowerUp's 55.2 x 58.4 mm is the GPU package, not the module.
- Heatsink: large black ribbed block with fan slots, per a baseboard heatsink listing. Fin count and layout unverified.
- Shroud/back, brand: none. Colours unverified.
- Generic fallback: a flat rectangular board with a box-fin heatsink block on top.
- Kind: server. Ref: NVIDIA-defined socket and module.
- Src: superb-tech.com (OAM vs SXM) ; en.wikipedia.org/wiki/SXM_(socket)

**nvidia-hgx-8gpu-baseboard** — HGX 8-GPU baseboard (H100 and H200; B200 variant)
- Form: baseboard, not a card. ~553 x 416 x 153 mm without connectors (OCP HGX spec rev 1.0 v0.1), ~565 mm with connectors (~).
- GPU layout: 8 SXM sockets, usually drawn as two rows of four (not verified against a diagram).
- NVLink/NVSwitch: H100 and H200 use 4 NVSwitch chips. B200 uses 2 NVLink Switch chips, moved to the board centre to shorten traces. Exact H100 chip placement unverified.
- Host links: one PCIe Gen5 x16 link per GPU.
- Heatsink: baseboard heatsink with fan slots (from a listing, unverified).
- Newer variants (Rubin NVL8, Blackwell Ultra) use the same 8-SXM concept.
- Kind: server. Ref: NVIDIA reference (OCP spec), built by OEMs.
- Src: opencompute.org (HGX baseboard spec r1 v0.1) ; servethehome.com (HGX B200 NVLink switch)

**nvidia-gb200-tray** — GB200 NVL72 compute tray (optional)
- Form: liquid-cooled tray in a 42U rack. 2 superchips per tray (1 Grace + 2 B200 each), so 4 GPUs per tray. 18 compute trays and 9 NVSwitch trays per rack.
- Lenovo's GB300 1U tray is 438 x 43.6 x 766 mm (OEM figure).
- Kind: server (rack). Ref: NVIDIA rack design, OEM-built trays.
- Src: lenovopress.lenovo.com (GB300 NVL72)

**nvidia-dgx-h100** — DGX H100 chassis (front)
- Form: 8U rack chassis, 356 mm H x 482.2 mm W x 897.1 mm L.
- Front: dark bezel hiding 8 U.2 NVMe bays (3.84 TB each) and 12 dual fan modules behind it. Front-panel LED detail unverified.
- Cooling: 12 dual fan modules, front-to-back, 1,105 CFM at 80% PWM.
- Colour: dark grey or black bezel (~).
- Kind: server. Ref: NVIDIA-built.
- Src: docs.nvidia.com DGX H100 user guide

**nvidia-dgx-b200** — DGX B200 chassis (front)
- Form: 444 mm H (~10U, derived), 482.2 x 897.1 mm.
- Front: bezel, 8 drive bays (vendor listing, ~). Layout and colour unverified.
- Cooling: front-to-back fan wall, count unverified.
- Kind: server. Ref: NVIDIA-built.
- Src: docs.nvidia.com DGX B200 user guide ; colfax-intl.com

**nvidia-dgx-station-gb300** — DGX Station GB300 (desk-side)
- Form: tower workstation. 1x B300 Blackwell Ultra, 72-core Grace CPU, 252 GB HBM3e and 496 GB LPDDR5X. 1,600 W on a 20 A circuit.
- Front I/O: 2x USB-C, 2x USB-A, audio. Rear: 2x QSFP112, 10 GbE, 1 GbE, PCIe Gen5 slots.
- Dimensions, colour, handles and fan layout: not found.
- Ref: NVIDIA design, built by OEMs (ASUS, HP, MSI, Supermicro, Dell, Exxact), so the look varies by OEM.
- Kind: workstation (desktop-class).
- Src: nvidia.com/en-us/products/workstations/dgx-station/

### 2.6 Edge and embedded

**nvidia-jetson-agx-orin-devkit** — Jetson AGX Orin developer kit
- Form: dev kit 110 x 110 x 71.7 mm including feet, carrier, module and thermal solution. Module 100 x 87 x 16 mm, 699-pin board-to-board with an integrated thermal transfer plate.
- Cooling: module heatsink with a fan on the kit. Fan size unverified.
- Colours: dark PCB with metal heatsink (~). I/O layout not verified.
- Generic fallback: near-square dark board with a silver finned block on one side.
- Kind: edge. Ref: yes (NVIDIA dev kit and carrier).
- Src: nvidia.com Jetson AGX Orin technical brief

**nvidia-jetson-orin-nano-super-devkit** — Jetson Orin Nano Super developer kit
- Form: 100 x 79 x 21 mm including feet. Module 69.6 x 45 mm, 260-pin SO-DIMM. Older Orin Nano kit was ~103 x 90.5 x 35 mm.
- I/O: 40-pin expansion header, 12-pin button header, 4-pin fan header, DC jack.
- Cooling: heatsink and fan fitted (fan size unverified).
- Colours: dark PCB, silver heatsink (~).
- Kind: edge. Ref: yes.
- Src: nvidia.com Jetson Orin page ; rs-online.com (945-13766-0000-000)

**nvidia-jetson-orin-nx-super-devkit** — Jetson Orin NX Super (module and carrier)
- Form: module 69.6 x 45 mm, 260-pin SO-DIMM (NVIDIA). No NVIDIA-built carrier confirmed; partner carriers exist (e.g. Yahboom, Waveshare). Carrier dimensions unverified.
- Generic fallback: use the Orin Nano dev kit footprint, with the SO-DIMM module as a small block on a dark board.
- Kind: edge. Ref: module yes; carrier partner-built.
- Src: nvidia.com Jetson Orin page ; category.yahboom.net (Jetson Orin NX)

**nvidia-jetson-agx-thor-devkit** — Jetson AGX Thor developer kit and T5000 module
- Form: dev kit 243.2 x 112.4 x 56.9 mm including feet, carrier, module and thermal solution. Module 100 x 87 mm, 699-pin, same footprint as the AGX Orin module.
- Cooling: thermal transfer plate with an optional fan or heatsink.
- Colours, I/O: unverified. Generic fallback: wide dark board (~2.2:1) with a bulky finned block on one side.
- Kind: edge. Ref: yes.
- Src: nvidia.com Jetson Thor page ; waveshare.com (AGX Thor dev kit)

**nvidia-igx-orin** — IGX Orin (dev kit and industrial system)
- Form: industrial module on an industrial carrier. Dimensions not found. Dev kit: 2,048-core Ampere GPU, 64 Tensor Cores, 12-core CPU (ASI Partner).
- Cooling: fanless or fan, unverified. I/O unverified.
- Generic fallback: rugged box with a finned side panel and a DIN-rail or wall-mount tab.
- Kind: edge. Ref: NVIDIA dev kit; partner systems only.
- Src: nvidia.com/en-us/edge-computing/products/igx/ ; asipartner.com (IGX Orin)

**nvidia-dgx-spark** — DGX Spark (GB10)
- Form: desktop mini box, 150 x 150 x 50.5 mm, 1.2 kg (NVIDIA).
- Rear ports: power button, 4x USB-C (one with power delivery), 1x HDMI 2.1a, 1x RJ-45 10 GbE, 2x QSFP (ConnectX-7, 200 GbE). Front ports not listed by NVIDIA.
- Power: 240 W external supply. GB10 TDP 140 W.
- Cooling: integrated thermal system; fan and vent layout unverified.
- Colour: unverified. Generic fallback: square grey box with a vent grille on the top or sides and a status light.
- Kind: workstation (desktop-class). Ref: NVIDIA-built; OEM variants exist.
- Src: docs.nvidia.com DGX Spark hardware ; servethehome.com (DGX Spark review)

**nvidia-jetson-nano** — Jetson Nano (original, fallback)
- Form: module 69.6 x 45 mm, 260-pin SO-DIMM. Dev kit board 100 x 80 x 29 mm (the brief's dimensions appear swapped).
- Cooling: passive heatsink on the module. Dev kit fan unverified.
- Kind: edge. Ref: yes.
- Src: NVIDIA Jetson Nano datasheet (DS-09366-001)

## 3. AMD

### 3.1 Consumer

Generic partner fallback: `amd-consumer-generic`, triple-fan open-air, ~300 x 130 x 60 mm (estimate).

**amd-rx7900xtx-mba** — Radeon RX 7900 XTX (AMD reference, MBA)
- Form: PCIe, full-height, full-length. 287 x 110 x 51 mm (AMD, TechPowerUp). Slot count ~2.5 (GamingPCBuilder, TechPowerUp forum); TechPowerUp's label says dual-slot. Not triple-slot as the brief implied.
- Cooling: open-air axial, ~2 fans (~). Fan diameter unverified.
- Shroud/back: black shroud, ~#1A1A1A (unverified). Red stripes named in the brief but not confirmed in any source read. Metal backplate unverified.
- Brand: "Radeon" logo placement unverified. No RGB assumed (unverified).
- Power: ~2x 8-pin (widely reported, not in the sources read).
- I/O: ~3x DP and 1x HDMI (not in the sources read). Bracket colour unverified.
- Kind: workstation. Ref: yes (AMD MBA).
- Src: techpowerup.com/gpu-specs/radeon-rx-7900-xtx.c3941 (excerpt only) ; amd.com RX 7900 XTX page ; gamingpcbuilder.com

**amd-rx7900xt-mba** — Radeon RX 7900 XT (AMD reference)
- Form: 276 x 110 x 51 mm (TechPowerUp excerpt; the reference status and dimensions are not fully confirmed).
- Cooling, shroud, power, I/O: as the XTX (~, unverified).
- Kind: workstation. Ref: believed yes, unverified.

**amd-rx6900xt-mba** — Radeon RX 6900 XT (AMD reference, MBA)
- Form: 267 x 120 x 50 mm (TechPowerUp). Slot count conflicts (TechPowerUp says triple-slot; 50 mm thickness suggests ~2.5).
- Cooling: open-air axial, fan count unverified.
- Shroud/back, brand, I/O: unverified. Power ~2x 8-pin (~, a TechPowerUp forum comment).
- Kind: workstation. Ref: yes (AMD product page returned 404; confirm the page before relying on it).
- Src: techpowerup.com/gpu-specs/radeon-rx-6900-xt.c3481

**amd-rx9070xt-generic** — Radeon RX 9070 XT (no AMD reference)
- No ref. AMD launched the RX 9070 series without AMD-built boards (TechPowerUp, article 333051). AMD's render was labelled "artistic render, not available for purchase".
- Generic: partner triple-slot card, typically three axial fans (~), ~330 x 130 x 65 mm (a single partner card excerpt, not a generic).
- Kind: workstation.
- Src: techpowerup.com/333051 ; overclock3d forum (MBA thread)

### 3.2 Workstation

Generic: `amd-workstation-generic`, dual-slot blower.

**amd-w7900-dual** — Radeon PRO W7900 Dual Slot
- Form: PCIe, full-height, 280 mm long, dual-slot (AMD).
- Cooling: active. Blower vs open-air not stated by AMD (~blower).
- Shroud/back, brand: unverified.
- Power: 2x 8-pin. 295 W TBP. 750 W PSU recommended (AMD).
- I/O: 3x DisplayPort 2.1 and 1x Enhanced Mini DisplayPort 2.1 (AMD). Standard-height bracket (~).
- Kind: workstation. Ref: yes (AMD).
- Src: amd.com RX W7900 dual-slot page ; techpowerup.com/323141

**amd-w7900** — Radeon PRO W7900 (triple-slot original)
- Form: 280 x 110 x 51 mm, triple-slot (TechPowerUp, AMD).
- Cooling: active. Open-air axial assumed from the 51 mm thickness (~).
- Power: 2x 8-pin. I/O: 3x DP 2.1 and 1x EDP 2.1.
- Shroud/back, brand: unverified.
- Kind: workstation. Ref: yes.
- Src: amd.com/en/products/graphics/workstations/radeon-pro/w7900.html

**amd-w7800** — Radeon PRO W7800
- Form: dual-slot, 280 x 110 x 40 mm (TechPowerUp).
- Cooling: blower (~, from an IgorsLab pre-launch article).
- Power, I/O, shroud, brand: unverified.
- Kind: workstation. Ref: yes.
- Src: techpowerup.com/gpu-specs/radeon-pro-w7800.c4148 ; igorslab.de (RX 7800 XT as W7800)

### 3.3 Data centre and server

**amd-mi210** — Instinct MI210 (PCIe)
- Form: PCIe Gen4 x16, dual-slot (~2 slots), full-height, full-length. 267 x 111 mm (TechPowerUp); AMD brochure gives 4.5 x 10.5 in.
- Cooling: fully passive, no fans.
- Shroud/back: none. Fin pattern unverified.
- Brand: none.
- Power: 1x 8-pin EPS12V. 300 W TDP.
- I/O: no display outputs. Bracket presence unverified.
- Modules: 3 Infinity Fabric links, 64 GB HBM2e.
- Kind: server. Ref: yes.
- Src: amd.com (Instinct MI210) ; AMD MI210 brochure (PDF)

**amd-mi250-oam** — Instinct MI250 and MI250X (OAM module)
- Form: OAM module, not a card. Dimensions not on AMD's page; the ~102 x 165 mm OAM envelope is assumed (~).
- Cooling: passive cold plate. Liquid-cooled variants exist (~).
- Modules: 8 Infinity Fabric links (AMD), ~100 GB/s each (~).
- 8-GPU UBB layout unverified.
- Kind: server. Ref: yes (AMD OAM).
- Src: amd.com/en/products/accelerators/instinct/mi200/mi250x.html

**amd-mi300x-oam** — Instinct MI300X (OAM module)
- Form: OAM module, 750 W, 192 GB HBM3, 8 XCD chiplets per module. Dimensions ~102 x 165 mm (~, unverified).
- Cooling: passive heatsink with a large fin array (~).
- Modules: 8 Infinity Fabric links at 128 GB/s each (AMD).
- Brand, power, I/O: OAM power comes from the baseboard. No card connector or display outputs (~).
- Kind: server. Ref: yes.
- Src: amd.com MI300X platform datasheet (PDF) ; lenovopress.lenovo.com (MI300X 8-GPU board)

**amd-ubb-8gpu** — MI300X 8-GPU Universal Baseboard (UBB 2.0)
- Form: baseboard with 8 OAM modules. Layout "2 rows of 4" not confirmed (~).
- Fabric: all 8 GPUs fully interconnected, 128 GB/s between each pair, 896 GB/s total (Lenovo Press).
- Kind: server. Ref: AMD UBB; built by OEMs.
- Src: lenovopress.lenovo.com (MI300X 8-GPU board) ; servethehome.com (MI325X 8-GPU baseboard)

### 3.4 Edge and embedded

**amd-versal-vek280** — Versal AI Edge (VEK280 evaluation kit)
- Form: full-size evaluation board, 240.5 x 186.4 mm, VE2802 device. No SOM. AMD publishes the board as the reference form.
- Cooling, colour, power, I/O: unverified.
- Kind: edge. Ref: yes (AMD evaluation board).
- Src: amd.com VEK280 page

**amd-kria-k26-som** — Kria K26 SOM
- Form: board-to-board SOM, 77 x 60 x 11 mm bare; 60 x 77 x 27 mm with heatsink (AMD DS986).
- Kind: edge. Ref: yes (AMD SOM).
- Src: docs.amd.com ds986 (KV260 starter kit, mechanical)

**amd-kv260** — Kria KV260 Vision AI starter kit
- Form: total 123 x 140 x 36 mm, carrier 123 x 140 x 23 mm (AMD DS986).
- The K26 SOM sits on the carrier (location ~ one end).
- Colour: revision dependent. Rev 1 has a red fan housing, Rev 2 black. Carrier PCB colour unverified.
- Cooling: active fan on the heatsink.
- I/O and power: unverified.
- Kind: edge. Ref: yes.
- Src: docs.amd.com ds986 ; element14.com (Kria K24/K26)

### 3.5 Integrated and mini PC

**amd-ryzen-ai-halo** — Ryzen AI Max (Strix Halo) developer mini PC
- Form: AMD's own developer box (announced at CES 2026, per AMD page). Box dimensions and cooling unverified. Comparable: Framework Desktop, 96.8 x 205.5 x 226.1 mm, 4.5 L (not AMD).
- Package: Strix Halo FP11, ~37.5 x 45 mm (leak-based, ~1,687 mm2, may not match the final part).
- Colour, ports, shroud: unverified.
- Kind: workstation (mini PC). Ref: yes for the AMD-built box; the generic Strix Halo mini PCs are partner-built.
- Src: amd.com (Ryzen AI Halo page) ; igorslab.de (Ryzen AI Halo box)

## 4. Intel

### 4.1 Consumer

Generic: `intel-consumer-generic`, dual-slot, 2 open-air fans, ~270 x 115 x 42 mm (estimate).

**intel-arc-a770-le** — Arc A770 Limited Edition
- Form: PCIe Gen4 x16, ~2-slot, full-height, full-length. 268.6 mm long (279.9 with bracket), ~111 mm card height including the PCIe connector, ~41 mm thick (Intel's labels are inconsistent, ~).
- Cooling: open-air, 2 axial fans, ~15 blades each (Wccftech, ~). Fan diameter unverified.
- Shroud/back: dark shroud, ~#141414. Screwless, with chamfered edges. Full backplate with matte silver accents (~#c8ccd1). LED rings around the fan bores.
- Brand and light: "ARC" logo on the side. RGB zones: one per fan bore, a strip along the top edge, and one zone not identified (TechPowerUp, from a snippet).
- Power: 1x 8-pin + 1x 6-pin PCIe. Connector position ~top edge.
- I/O: ~3x DP and 1x HDMI (unverified). Bracket colour unverified.
- Kind: workstation. Ref: yes (Intel-built).
- Src: intel.com support 000092554 (dimensions) ; intel.com support 000092523 (power) ; techpowerup.com/298989 (RGB zones)

**intel-arc-a750-le** — Arc A750 Limited Edition
- Form: assumed the same as the A770 LE (~, Intel gives one dimensions article for both).
- Cooling, shroud: same family (~). Power: 8-pin + 6-pin.
- I/O: unverified.
- Kind: workstation. Ref: yes (Intel-built).
- Src: intel.com support 000092554 ; 000092523

**intel-arc-b580-le** — Arc B580 Limited Edition (BMG-G21, 12 GB)
- Form: PCIe, ~2.3-slot, full-height, full-length. ~272 x 115 x 44 mm (Microcenter, eBay). Thickness conflicts: 38, 44 and 45 mm across sources (Club386, TechPowerUp).
- Cooling: open-air, 2 fans with a metallic hub (Club386). Fan diameter unverified (an 80 mm claim is unconfirmed).
- Shroud/back: black rubberised fan faces with no LED surround, set lower than the faceplate. Silver backplate lettering (~#c8ccd1). Sloped heatsink cut-out.
- Brand and light: white "intel ARC" LED on the side edge near the PCIe connector. No RGB.
- Power: 1x 8-pin PCIe (not 12V-2x6). 190 W TBP.
- I/O: 3x DP 2.1 and 1x HDMI 2.1 (Club386).
- Kind: workstation. Ref: yes (the only B-series LE Intel makes in-house).
- Src: club386.com (B580 LE review) ; techpowerup.com/gpu-specs/arc-b580.c4244 (snippet)

**intel-arc-b570-generic** — Arc B570 (no Intel reference)
- No ref: partner-only (Club386). Use the generic consumer form.
- Kind: workstation.

### 4.2 Workstation

**intel-arc-pro-b60** — Arc Pro B60 (Battlemage, 24 GB; 48 GB dual-GPU variant)
- Maker: no Intel-built card. Intel publishes a reference I/O layout and sells none itself; B60 boards come from partners (Sparkle, ASRock and others). Ref: no, so the partner look varies.
- Form: 2-slot, full-height. TechPowerUp gives 167 x 69 x 40 mm, which does not reconcile with other listings (~, unverified).
- Cooling by SKU: Sparkle sells a 24 GB blower, a 24 GB passive card and a 48 GB dual-GPU fan-cooled card. ASRock's Creator is a 2-slot blower. Others use passive to triple-fan.
- Shroud, backplate, light: partner-specific. No shared Intel shroud.
- Power: per SKU, unverified.
- I/O: DP plus HDMI per Intel's layout. Partners may swap HDMI for a second DP (Intel support 000099073).
- Generic fallback: `intel-workstation-generic`, 2-slot blower, full-height.
- Kind: workstation.
- Src: intel.com support 000099073 ; asrock.com (B60 Creator) ; jonpeddie.com (Sparkle dual-GPU)

**intel-arc-pro-a60** — Arc Pro A60 (Alchemist, 12 GB)
- Maker: unconfirmed (Intel or partner). Intel announced the A60 and A60M in June 2023.
- Form: single-slot, full-height ("slim single slot, full height" per Intel's brochure). Retailer listing 241 x 111 x 20 mm (~).
- Cooling: unverified; generic is a single-slot blower.
- Shroud, backplate, light, I/O: unverified. Power ~130 W (unconfirmed forum post).
- Kind: workstation. Ref: uncertain.
- Src: intel.com newsroom (Arc Pro A60 announcement) ; tomshardware.com (A60 single-slot)

**intel-max1100** — Data Center GPU Max 1100 (Ponte Vecchio, 48 GB HBM2e)
- Form: PCIe Gen5 x16 card. ~267 mm long, dual-slot, full-height (~; TechPowerUp only).
- Cooling: passive, for server airflow (~). No fan.
- Shroud/back: none. Bare finned heatsink (~).
- Brand: none. Bracket: blank, no display outputs.
- Power: conflicting. TechPowerUp says 1x 12-pin; TweakTown (2022) says 16-pin 12VHPWR. Unconfirmed.
- Kind: server. Ref: yes (Intel-branded; Intel lists a discontinuance date of Jan 2026).
- Src: intel.com specifications 232876 ; techpowerup.com (Max 1100) ; tweaktown.com/news/89438

### 4.3 Data centre and server

**intel-gaudi3-oam** — Gaudi 3 OAM module (HL-325L) on the HLB-325 baseboard
- Form: OAM 2.0 mezzanine module, ~102 x 165 mm (~, the OAM 2.0 envelope, not in the sources read). 8 HBM2e stacks, 128 GB, 900 W TDP.
- Cooling: passive heatsink with air, up to 900 W (Intel). A liquid-cooled sibling, HL-335, is named by one source only (~).
- Shroud/back: none. Bare heatsink, no branding.
- Power: via the OAM pins on the baseboard. No discrete connector.
- Baseboard: HLB-325 holds 8 OAMs. Each has 24x 200 GbE RoCE. 21 links per chip go all-to-all inside the board. Each card sends 3x 200 GbE out. Layout 2 rows of 4 (~). 6x OSFP800 external ports on the baseboard (~).
- Kind: server. Ref: yes (Intel OAM and HLB-325 Universal Baseboard).
- Src: cdrdv2-public.intel.com (Gaudi 3 white paper) ; gigacomputing.com (Intel Gaudi)

**intel-gaudi3-pcie** — Gaudi 3 PCIe card (HL-338)
- Form: PCIe Gen5 x16, full-height, dual-slot, 10.5 in (~267 mm) long, 128 GB HBM2e.
- Cooling: passive, up to 600 W, needs chassis airflow (Intel, ServeTheHome).
- Shroud/back: none. Bare finned heatsink (~).
- Brand: none. Bracket: standard, no display outputs (~).
- Power: unverified (~8-pin pair or 12V-2x6).
- Kind: server. Ref: yes (Intel product brief 817488). Gigabyte also sells one.
- Src: intel.com content-details 817488 ; servethehome.com (Gaudi 3 GA)

**intel-flex170** — Flex 170 (16 GB GDDR6)
- Form: PCIe Gen4 x16, full-height, single-wide (Cisco and Lenovo) per the owner's brief. Other sources say dual-slot (TechPowerUp), and length conflicts (Cisco 3/4 length, Lenovo full length). Not low-profile: that is the Flex 140.
- Cooling: passive, 150 W (Lenovo).
- Shroud/back: none. Bare finned heatsink (~).
- Power: 1x 8-pin auxiliary (TechPowerUp). Lenovo supplies a 450 mm auxiliary cable for the SE450 (~).
- I/O: none (Intel lists 0 displays).
- Kind: server. Ref: yes (Intel-built; Lenovo and Cisco ship it).
- Src: lenovopress.lenovo.com (lp1829) ; cisco.com (UCS X-Series Flex white paper) ; techpowerup.com/gpu-specs/data-center-gpu-flex-170.c4071

### 4.4 Edge and integrated

**intel-coreultra-igpu-nuc** — Core Ultra integrated Arc iGPU in NUC-class mini PCs
- Form: a small box, not a card. Intel-built chassis: none now (the NUC brand moved to ASUS).
- Package: Arrow Lake-H (Core Ultra 200H) is a multi-tile package. The graphics tile (Arc 140T) is a separate die beside the compute tile (~).
- Box sizes (ASUS NUC 15): Pro Slim 117 x 112 x 37 mm, Pro Tall 117 x 112 x 54 mm (W x D x H incl. feet). NUC 15 Pro+ 144 x 112 x 41.8 mm.
- Lunar Lake (Core Ultra 200V) puts memory on the package (general knowledge, unverified here).
- Generic fallback: small square box with a top or side vent grille.
- Kind: edge (mini PC). Ref: ASUS-built NUC; no Intel reference chassis.
- Src: asus.com NUC 15 Pro specs ; wikipedia (Arrow Lake)

**intel-arc-laptop-dgpu** — Arc laptop dGPU (A770M class)
- Form: BGA GPU die soldered to the board, GDDR6 chips on the same PCB around the die (~, typical layout, not verified by teardown). A770M: 16 GB GDDR6, 256-bit, 1650 MHz (TechPowerUp).
- Generic: a square BGA die with a ring of 8 memory chips (~). The desktop A770 LE uses 8 GDDR6 modules around its ACM-G10 die (Wccftech).
- Cooling, shroud: laptop heatsink and fans, unverified.
- Kind: laptop. Ref: none (OEM design).
- Src: techpowerup.com/gpu-specs/arc-a770m.c3909 ; wccftech.com (A770 LE breakdown)

### 4.5 Apple Silicon (integrated, no card)

**apple-m4-soc** — Apple M4 Pro and M4 Max
- Form: SoC package with unified memory (memory on or beside the package, ~, general knowledge). No discrete GPU card.
- Mac mini (M4 Pro): 127 x 127 x 50 mm (Apple). Kind `workstation`.
- Mac Studio (M4 Max): 197 x 197 x 95 mm (Apple). Kind `workstation`.
- MacBook Pro 14-inch (M4 Pro and M4 Max): 312.6 x 221.2 x 15.5 mm (Apple). Kind `laptop`.
- Models already exist: `exterior/apple-laptop.json`, `exterior/apple-desktop.json`, `board/apple-soc.json`.
- Src: support.apple.com/en-us/121555 (Mac mini) ; support.apple.com/en-us/122211 (Mac Studio) ; support.apple.com/en-us/121553 (MacBook Pro 14)

## 5. Matching table

Detection reports a GPU name, and the name does not say whether a card is the vendor's reference board or a partner card. The rule below uses the name. If the PCI subsystem vendor is available, a subsystem vendor of `10de` (NVIDIA) probably marks an FE board. Verify on real hardware before relying on it.

| Detected name (pattern) | Card id | Notes |
|---|---|---|
| NVIDIA GeForce RTX 5090 | nvidia-rtx5090-fe | Default to FE for the name. |
| NVIDIA GeForce RTX 5080 | nvidia-rtx5080-fe | |
| NVIDIA GeForce RTX 5070 Ti | nvidia-rtx5070ti-generic | Partner-only. |
| NVIDIA GeForce RTX 5070 | nvidia-rtx5070-fe | |
| NVIDIA GeForce RTX 4090 | nvidia-rtx4090-fe | Example from the brief. |
| NVIDIA GeForce RTX 4080 SUPER | nvidia-rtx4080-fe | Same board assumed. |
| NVIDIA GeForce RTX 4080 | nvidia-rtx4080-fe | |
| NVIDIA GeForce RTX 4070 Ti (SUPER) | nvidia-rtx4070ti-generic | Partner-only. |
| NVIDIA GeForce RTX 4070 | nvidia-rtx4070-fe | |
| NVIDIA GeForce RTX 3090 | nvidia-rtx3090-fe | |
| NVIDIA RTX PRO 6000 Blackwell Max-Q | nvidia-rtxpro6000-maxq | Check "Max-Q" first. |
| NVIDIA RTX PRO 6000 Blackwell | nvidia-rtxpro6000-ws | Workstation Edition. |
| NVIDIA RTX 6000 Ada | nvidia-rtx6000ada | |
| NVIDIA RTX 4000 Ada | nvidia-rtx4000ada | Single slot. |
| NVIDIA RTX A6000 | nvidia-rtxa6000 | |
| NVIDIA H100 PCIe | nvidia-h100-pcie | Server, PCIe. |
| NVIDIA H100 NVL | nvidia-h100-nvl | |
| NVIDIA H200 NVL | nvidia-h200-nvl | |
| NVIDIA H100 80GB HBM3 (SXM, in a server) | nvidia-sxm5-module | Name alone is ambiguous. Use the form factor: SXM in an HGX/DGX chassis is the module. |
| NVIDIA H200 (SXM) | nvidia-sxm5-module | |
| NVIDIA HGX H100/H200 baseboard | nvidia-hgx-8gpu-baseboard | |
| NVIDIA DGX H100 | nvidia-dgx-h100 | System. |
| NVIDIA DGX B200 | nvidia-dgx-b200 | System. |
| NVIDIA DGX Station (GB300) | nvidia-dgx-station-gb300 | Workstation. |
| NVIDIA DGX Spark, GB10 | nvidia-dgx-spark | Desktop-class. |
| NVIDIA L40S | nvidia-l40s | |
| NVIDIA L40 | nvidia-l40 | |
| NVIDIA L4 | nvidia-l4 | |
| NVIDIA A100 PCIe | nvidia-a100-pcie | |
| NVIDIA A40 | nvidia-a40 | |
| NVIDIA A10 | nvidia-a10 | |
| NVIDIA A2 | nvidia-a2 | |
| NVIDIA T4 | nvidia-t4 | |
| NVIDIA Jetson AGX Orin | nvidia-jetson-agx-orin-devkit | |
| NVIDIA Jetson Orin Nano (Super) | nvidia-jetson-orin-nano-super-devkit | |
| NVIDIA Jetson Orin NX | nvidia-jetson-orin-nx-super-devkit | |
| NVIDIA Jetson AGX Thor | nvidia-jetson-agx-thor-devkit | |
| NVIDIA IGX Orin | nvidia-igx-orin | |
| NVIDIA Jetson Nano | nvidia-jetson-nano | Fallback. |
| AMD Radeon RX 7900 XTX | amd-rx7900xtx-mba | |
| AMD Radeon RX 7900 XT | amd-rx7900xt-mba | |
| AMD Radeon RX 6900 XT | amd-rx6900xt-mba | |
| AMD Radeon RX 9070 XT | amd-rx9070xt-generic | Partner-only. |
| AMD Radeon PRO W7900 Dual Slot | amd-w7900-dual | |
| AMD Radeon PRO W7900 | amd-w7900 | |
| AMD Radeon PRO W7800 | amd-w7800 | |
| AMD Instinct MI210 | amd-mi210 | |
| AMD Instinct MI250 / MI250X | amd-mi250-oam | |
| AMD Instinct MI300X | amd-mi300x-oam | |
| AMD Instinct MI300X in an 8-GPU board | amd-ubb-8gpu | |
| AMD Versal AI Edge (VEK280) | amd-versal-vek280 | |
| AMD Kria K26 SOM | amd-kria-k26-som | |
| AMD Kria KV260 | amd-kv260 | |
| AMD Ryzen AI Max (Strix Halo), Ryzen AI Halo | amd-ryzen-ai-halo | Integrated, mini PC. |
| Intel Arc A770 | intel-arc-a770-le | |
| Intel Arc A750 | intel-arc-a750-le | |
| Intel Arc B580 | intel-arc-b580-le | |
| Intel Arc B570 | intel-arc-b570-generic | Partner-only. |
| Intel Arc Pro B60 | intel-arc-pro-b60 | Partner-only. |
| Intel Arc Pro A60 | intel-arc-pro-a60 | Builder unconfirmed. |
| Intel Data Center GPU Max 1100 | intel-max1100 | |
| Intel Gaudi 3 (OAM, in a baseboard) | intel-gaudi3-oam | |
| Intel Gaudi 3 (PCIe, HL-338) | intel-gaudi3-pcie | |
| Intel Flex 170 | intel-flex170 | |
| Intel Core Ultra (Arc iGPU, NUC) | intel-coreultra-igpu-nuc | Integrated. |
| Intel Arc laptop (A770M, Arc laptop GPU) | intel-arc-laptop-dgpu | Laptop. |
| Apple M3 Pro / M4 Pro / M4 Max (any Apple M-series) | apple-m4-soc | Integrated; existing Apple models. |

Fallbacks per vendor and class, used when no exact id matches:
- NVIDIA consumer: `nvidia-consumer-generic`. Workstation: `nvidia-workstation-generic`. Data centre PCIe: `nvidia-dc-passive-dual` (dual-slot passive) or `nvidia-dc-passive-lp` (single-slot low-profile). Module: `nvidia-sxm5-module`.
- AMD consumer: `amd-consumer-generic`. Workstation: `amd-workstation-generic` (blower). Data centre: `amd-oam-generic`.
- Intel consumer: `intel-consumer-generic`. Workstation: `intel-workstation-generic` (blower). Data centre: `intel-dc-generic` (passive).
- Edge: `generic-edge-board` (dark PCB with heatsink). Desktop mini box: `generic-mini-box`.
- Class fallbacks: `consumer-generic`, `workstation-generic`, `datacentre-generic`, `edge-generic`, `laptop-generic`.

If the name matches several rows, prefer the most specific pattern (e.g. "Max-Q" before "RTX PRO 6000", "SUPER" before the base name).

## 6. Modelling notes

### 6.1 Scale and form

- Model each card as its real bounding box at 1 unit = 10 cm: 304 x 137 x 40 mm becomes 3.0 x 1.4 x 0.4 units. Resolve the robot-width conflict in section 0 first.
- Build the card as a box stack: PCB slab, shroud, fan rings, backplate, I/O bracket. Use cylinders for fans and heatsink fins. Keep the fin count low (about 6 to 12 visible fins).
- Dual-fan open-air: two cylinders as fan rings, each with a dark hub and 7 or 9 blade stubs. Triple-fan: three rings in a row.
- Passive data-centre card: a flat finned block (a stack of thin boxes) with no fan and an empty bracket.
- SXM module: a flat rectangular slab with a box-fin heatsink block on top. HGX baseboard: a green PCB rectangle (`mask-green`) with 8 square sockets in two rows of four, and a large heatsink shroud on top.
- Edge boards: near-square dark PCB with a silver heatsink block and a small fan. DGX Spark: a 15 x 15 x 5 unit square box, scaled to 10 cm, with a vent grille.

### 6.2 Recognisability, most to least important

1. Overall silhouette: length, height, and whether the card is a 2-slot or 3-slot form.
2. Fan count and layout: dual flow-through (NVIDIA FE), open-air triple (AIB look), blower with its bracket-end fan, passive fin stack.
3. Shroud colour and light position: the Intel A770 LE light bar on the side and the top edge; the Intel B580 side LED; the AMD black shroud with red accent (if confirmed).
4. Backplate: full backplate, silver accents, or none.
5. Power connector: 12V-2x6 position, 8-pin pair, or no connector at all (passive and slot-powered cards).
6. I/O bracket: 3 or 4 display outputs, or a blank bracket for data-centre cards.

### 6.3 Palette

Reuse `assets/hardware/palette.json` names where they fit. Suggested values, with hex estimates unless sourced:

| Name | Use | Hex (estimate) |
|---|---|---|
| `gpu-shroud` | NVIDIA FE graphite shroud | #2A2A2A |
| `gpu-shroud` (alt) | 4090 FE black shroud | #1A1A1A |
| `gpu-accent` | NVIDIA light or logo | #76B900 (NVIDIA green, not sourced here) |
| `case-accent` | AMD red accent (7900 XTX, unconfirmed) | #C8102E |
| `rgb` / `led-signal` | Intel ARC light (white LED on B580; RGB on A770) | #FFFFFF / #0071C5 (Intel blue, estimate) |
| `heatsink` | fins and heat pipes | #9AA3AB |
| `aluminium` | bare fin stacks, OAM heatsinks | #C3C8CC |
| `fan` / `fan-blade` | fan rings and hubs | #1A1D1F / #2B3038 |
| `pcb-black` | Jetson and edge boards | #1A1D1B |
| `mask-green` | HGX baseboard and standard PCBs | #14532D |
| `metal-dark` | passive server cards, bare fin stacks | #4A5056 |
| `nickel` | backplate accents | #B0B5BA |

### 6.4 Existing parts to reuse

From `assets/hardware/parts/`:
- `gpu-pcie.json`: base card body.
- `fan-120.json`, `blower-fan.json`: open-air fan and blower.
- `heatpipe-straight.json`: heat pipes on dual-slot cards.
- `io-shroud.json`: I/O bracket.
- `pcie-8pin.json`, `12vhpwr.json`: power connectors (`eps8.json` for EPS 8-pin).
- `pcie-x16-slot.json`: slot.
- `heatsink-tower.json`, `vrm-heatsink.json`: heatsinks.
- No light-strip part exists. Build RGB or light strips from a box in the palette colour `rgb`.

Check the schema in `docs/design/hardware-plazas.md` before writing new parts.

### 6.5 Open points for the owner

- Scale: the 10 cm unit for GPUs against the app's robot-width unit (section 0).
- Unverified details to check before the modeller starts: the 4090 FE X-shaped frame, the 7900 XTX red stripes, the 5090 FE fan count and diameter, the 4080 FE fan count, and the A770 LE RGB zones.
- Sources: most fields come from search excerpts or short page reads. Before the high-detail cards (5090 FE, 4090 FE, A770 LE, B580 LE, RTX PRO 6000) are modelled, open the cited pages and confirm the cooler and shroud details.

## 7. Source list

- NVIDIA: nvidia.com GeForce 50/40/30 series pages; NVIDIA RTX PRO and RTX Ada workstation datasheets; NVIDIA data-centre product briefs (H100 PCIe PB-11133-001, H100 NVL PB-11773-001, L4 PB-11316-001, A100 PB-10577-001, A10 PB-10415-001, A2 product brief, T4 product brief); NVIDIA DGX H100, DGX B200 and DGX Spark user guides; NVIDIA Jetson Orin and Thor pages.
- AMD: amd.com Radeon RX 7900 XTX, Radeon PRO W7900 and W7900 Dual Slot, Instinct MI210, MI250X and MI300X pages; AMD MI300X platform datasheet; AMD DS986 (KV260); AMD VEK280 page; Ryzen AI Halo page.
- Intel: intel.com support 000092554 (A770/A750 LE dimensions), 000092523 (power), 000099073 (Arc Pro B60); intel.com Gaudi 3 white paper and product brief 817488; Data Center GPU Max 1100 specifications.
- Reviews and databases: techpowerup.com GPU database; servethehome.com; club386.com (B580 LE); lanoc.org (RTX 5070 FE); jonpeddie.com (RTX PRO 6000); lenovopress.lenovo.com; cisco.com (Flex 170); opencompute.org (HGX baseboard spec); gamingpcbuilder.com.
