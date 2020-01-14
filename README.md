# vongform

Generate umbrella chart `requirements.yaml` and `values.yaml` files for helm
using consul kv. Wow, that's not niche at all!

```
$ vong --set <service>=<version> --rm <service> --output <dir>
```

Considers the environment variables `VONGFORM_OUTPUT_DIR` and `VONGFORM_DEFAULT_REPOSITORY`.

The help output:

```
vongform 0.1.0
Manage data for a helm umbrella chart stored in consul. Update service versions and emit the chart.

USAGE:
    vongform [OPTIONS]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -o, --output <output>            output the umbrella chart to the given directory;
                                     checks VONGFORM_OUTPUT_DIR and falls back to `./chart'
    -r, --repository <repository>    the fully-qualified url of the helm chart repository to use; defaults to
                                     VONGFORM_DEFAULT_REPOSITORY
        --set <set>...               set a service to a version; can be repeated:
                                     vong --set sessions-2020=1.0.0 --set auth-2020=1.2.3
```

## TODO

`rm` is not yet implemented.
