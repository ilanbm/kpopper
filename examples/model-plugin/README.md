# Minimal read-model source repository

This is a source fixture for the kpop-model package contract. Copy the complete directory contents, including hidden native history and attributes, into a dedicated Git repository before building. It is not itself an installable plugin: it has no generated package lock, common launcher, engine archive or Codex plugin manifest. The release-qualified prebuilt kpop-model builder creates those.

The dedicated native model records a fictional venue's quoted standard rate and that the quote leaves weekend pricing unspecified. The exact synthetic quote is under knowledge/venue-rates/sources/. The GROUNDING.yaml and native history were created with kpop 0.15.1; generated history must not be edited by hand.

The descriptor names a read-only package with one native smoke entry. A reader should cite the source and preserve the model's scope. This fixture is for package-contract checks; it does not demonstrate product usefulness, broad coverage or model efficacy. The independent evaluator oracle is kept elsewhere and is not part of this source repository.

After committing the copied source repository, build with the qualified prebuilt tool and complete engine archive:

    kpop-model build --source /ABS/SOURCE_REPO --descriptor model-package.json --engine-archive /ABS/kpopper-0.15.1-darwin-arm64.tar.gz --output /ABSENT/venue-rates-0.1.0

Then validate and set up only the generated bundle:

    kpop-model --bundle /ABS/venue-rates-0.1.0 verify
    kpop-model --bundle /ABS/venue-rates-0.1.0 --cache /ABS/PRIVATE-CACHE setup
    kpop-model --bundle /ABS/venue-rates-0.1.0 --cache /ABS/PRIVATE-CACHE pull pricing.standard_hourly_rate pricing.weekend_rate_status

The package builder and complete engine archive must already be available as qualified prebuilt artifacts. The commands above are the reviewed contract interface; a built and setup package, not this source folder, is required before claiming that the consumer route works. Model authors do not install Rust or compile the common tool.
