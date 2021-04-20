
// Use ES module import syntax to import functionality from the module
// that we have compiled.
//
// Note that the `default` import is an initialization function which
// will "boot" the module and make it ready to use. Currently browsers
// don't support natively imported WebAssembly as an ES module, but
// eventually the manual initialization won't be required!
//
// Specifically for citeproc-rs, note that we load the JS file from the
// _web directory. The other directories are for different environments
// like Node and WebPack. You could also load from cdnjs or similar with a
// version number and the /_web/citeproc_rs_wasm.js file path.
console.log("first");
const { Driver } = wasm_bindgen;

const style = '<?xml version="1.0" encoding="utf-8"?>\n<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" version="1.0">\n<citation><layout><group delimiter=" "><text variable="title" /><text term="edition" form="long"/></group></layout></citation></style>';

class Fetcher {
    async fetchLocale(lang) {
        // We're going to return a sentinel value so we know the french locale is getting loaded
        let loc = '<?xml version="1.0" encoding="utf-8"?><locale xml:lang="' + lang + '"><terms><term name="edition">édition (fr)</term></terms></locale>';
        return loc;
    }
}

async function run() {
    // First up we need to actually load the wasm file, so we use the
    // default export to inform it where the wasm file is located on the
    // server, and then we wait on the returned promise to wait for the
    // wasm to be loaded.
    //
    // It may look like this: `await init('./pkg/without_a_bundler_bg.wasm');`,
    // but there is also a handy default inside `init` function, which uses
    // `import.meta` to locate the wasm file relatively to js file.
    //
    // Note that instead of a string you can also pass in any of the
    // following things:
    //
    // * `WebAssembly.Module`
    //
    // * `ArrayBuffer`
    //
    // * `Response`
    //
    // * `Promise` which returns any of the above, e.g. `fetch("./path/to/wasm")`
    //
    // This gives you complete control over how the module is loaded
    // and compiled.
    //
    // Also note that the promise, when resolved, yields the wasm module's
    // exports which is the same as importing the `*_bg` module in other
    // modes
    try {
        console.log('fetching wasm');
        await wasm_bindgen('../../pkg-nomod/_no_modules/citeproc_rs_wasm_bg.wasm');
    } catch (e) {
        console.log('failing hard');
        document.write('<p id="failure">' + e.message + '</p>');
        return;
    }

    // And afterwards we can use all the functionality defined in wasm.
    // const result = Driver.new("<>noparse", {}, 2);
    const fetcher = new Fetcher();
    const driver = Driver.new(style, fetcher, "html");
    if (driver == null) {
        throw new Error("wasm Driver.new doesn't work!");
    }

    console.log("--- Successfully loaded wasm driver. You can now use it. ---")
    driver.insertReferences([{id: "citekey", title: "Hello", language: 'fr-FR'}]);
    driver.initClusters([{id: 1, cites: [{id: "citekey"}]}]);
    driver.setClusterOrder([ {id: 1} ]);
    await driver.fetchLocales();
    let result = driver.builtCluster(1);
    console.log("Built a cite cluster:", result);
    document.write('<p id="success">success</p>')
    return;
}

run();
