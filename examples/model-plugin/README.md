# Venue-rates model example

**If this README is inside a generated plugin** with `model-package.lock.json` and
`bin/kpop-model`, the package is ready to install. It includes its engine and
venue-rates skill. After installation, ask that skill for the standard room rate
and whether weekend pricing is known. No separate kpopper or compiler is needed.

**In the source checkout**, this directory is a fixture for the package contract.
It has no generated lock, launcher, engine archive or plugin manifest yet. Copy its
complete contents, including hidden native history and attributes, into a dedicated
Git repository before building with the qualified prebuilt `kpop-model` tool.

Follow the [complete walkthrough](https://github.com/ilanbm/kpopper/blob/main/docs/model-plugins.md#try-the-complete-example)
for source creation, packaging and local Codex installation. Initial support is
macOS ARM64; the prebuilt companion currently comes from a maintainer and has not
been publicly released. Use this fixture in a fresh source directory, separately
from any model you have authored yourself.

The dedicated native model records a fictional venue's quoted standard rate and that the quote leaves weekend pricing unspecified. The exact synthetic quote is under knowledge/venue-rates/sources/. The GROUNDING.yaml and native history were created with kpop 0.15.1; generated history must not be edited by hand.

The descriptor names a read-only package with one native smoke entry. A reader should cite the source and preserve the model's scope. This fixture is for package-contract checks; it does not demonstrate product usefulness, broad coverage or model efficacy. The independent evaluator oracle is kept elsewhere and is not part of this source repository.

After committing the copied source repository, build with the qualified prebuilt tool and complete engine archive:

    kpop-model build --source /ABS/SOURCE_REPO --descriptor model-package.json --engine-archive /ABS/kpopper-0.15.1-darwin-arm64.tar.gz --output /ABSENT/venue-rates-0.1.0

Then validate and set up only the generated bundle:

    kpop-model --bundle /ABS/venue-rates-0.1.0 verify
    kpop-model --bundle /ABS/venue-rates-0.1.0 --cache /ABS/PRIVATE-CACHE setup
    kpop-model --bundle /ABS/venue-rates-0.1.0 --cache /ABS/PRIVATE-CACHE pull pricing.standard_hourly_rate pricing.weekend_rate_status

The package builder and complete engine archive must already be available as qualified prebuilt artifacts. The commands above are the reviewed contract interface; a built and setup package, not this source folder, is required before claiming that the consumer route works. Model authors do not install Rust or compile the common tool.
