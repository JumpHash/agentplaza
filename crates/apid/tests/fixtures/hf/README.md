---
base_model: distilbert-base-uncased-finetuned-sst-2-english
library_name: transformers.js
---

https://huggingface.co/distilbert-base-uncased-finetuned-sst-2-english with ONNX weights to be compatible with Transformers.js.

## Usage (Transformers.js)

If you haven't already, you can install the [Transformers.js](https://huggingface.co/docs/transformers.js) JavaScript library from [NPM](https://www.npmjs.com/package/@huggingface/transformers) using:
```bash
npm i @huggingface/transformers
```

You can then use the model to classify text like this:

```js
import { pipeline } from "@huggingface/transformers";

// Create a sentiment analysis pipeline
const classifier = await pipeline('sentiment-analysis', 'Xenova/distilbert-base-uncased-finetuned-sst-2-english');

// Classify input text
const output = await classifier('I love transformers!');
console.log(output);
// [{ label: 'POSITIVE', score: 0.999788761138916 }]

// Classify input text (and return all classes)
const output2 = await classifier('I love transformers!', { top_k: null });
console.log(output2);
// [
//   { label: 'POSITIVE', score: 0.999788761138916 },
//   { label: 'NEGATIVE', score: 0.00021126774663571268 }
// ]
```

---

Note: Having a separate repo for ONNX weights is intended to be a temporary solution until WebML gains more traction. If you would like to make your models web-ready, we recommend converting to ONNX using [🤗 Optimum](https://huggingface.co/docs/optimum/index) and structuring your repo like this one (with ONNX weights located in a subfolder named `onnx`).