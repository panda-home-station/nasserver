const args = sys.args || [];
const system = sys.system;

if (args.length === 0) {
    try {
        const stats = JSON.parse(system.stats());
        print(JSON.stringify(stats, null, 2));
    } catch (e) {
        print(`sysinfo: failed to get stats: ${e}`);
    }
} else {
    const cmd = args[0];
    switch (cmd) {
        case "health":
            print(system.health());
            break;
        case "device":
            try {
                const dev = JSON.parse(system.device());
                print(JSON.stringify(dev, null, 2));
            } catch (e) {
                print(`Error getting device info: ${e}`);
            }
            break;
        case "gpu":
            try {
                const gpu = JSON.parse(system.gpu());
                print(JSON.stringify(gpu, null, 2));
            } catch (e) {
                print(`Error getting gpu info: ${e}`);
            }
            break;
        case "ports":
            if (args.length < 2) {
                print("sysinfo ports: requires port numbers");
            } else {
                const ports = args.slice(1).map(p => parseInt(p)).filter(p => !isNaN(p));
                try {
                    const res = JSON.parse(system.checkPorts(ports));
                    print(JSON.stringify(res, null, 2));
                } catch (e) {
                    print(`Error checking ports: ${e}`);
                }
            }
            break;
        case "docker-mirrors":
            if (args.length < 2) {
                print("sysinfo docker-mirrors: requires subcommand [get|set]");
            } else {
                const sub = args[1];
                if (sub === "get") {
                    try {
                        const mirrors = JSON.parse(system.getDockerMirrors());
                        print(JSON.stringify(mirrors, null, 2));
                    } catch (e) {
                        print(`Error getting docker mirrors: ${e}`);
                    }
                } else if (sub === "set") {
                     if (args.length < 3) {
                        print("sysinfo docker-mirrors set: requires mirrors json array");
                    } else {
                        const jsonStr = args[2];
                        try {
                            // Validate JSON first
                            const mirrors = JSON.parse(jsonStr);
                            if (!Array.isArray(mirrors)) throw "Mirrors must be an array";
                            system.setDockerMirrors(jsonStr);
                            print("Docker mirrors updated");
                        } catch (e) {
                            print(`Error setting docker mirrors: ${e}`);
                        }
                    }
                } else {
                    print(`sysinfo docker-mirrors: unknown subcommand '${sub}'`);
                }
            }
            break;
         case "docker-settings":
            if (args.length < 2) {
                print("sysinfo docker-settings: requires subcommand [get|set]");
            } else {
                const sub = args[1];
                if (sub === "get") {
                    try {
                        const settings = JSON.parse(system.getDockerSettings());
                        print(JSON.stringify(settings, null, 2));
                    } catch (e) {
                        print(`Error getting docker settings: ${e}`);
                    }
                } else if (sub === "set") {
                     if (args.length < 3) {
                        print("sysinfo docker-settings set: requires settings json object");
                    } else {
                        const jsonStr = args[2];
                        try {
                            system.setDockerSettings(jsonStr);
                            print("Docker settings updated");
                        } catch (e) {
                            print(`Error setting docker settings: ${e}`);
                        }
                    }
                } else {
                    print(`sysinfo docker-settings: unknown subcommand '${sub}'`);
                }
            }
            break;
        default:
            print(`sysinfo: unknown subcommand '${cmd}'`);
    }
}
