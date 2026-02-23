const args = sys.args || [];
const process = sys.process;

if (args.length === 0) {
    try {
        process.chdir("~");
    } catch (e) {
        print(`cd: ${e}`);
    }
} else {
    try {
        process.chdir(args[0]);
    } catch (e) {
        print(`cd: ${e}`);
    }
}
