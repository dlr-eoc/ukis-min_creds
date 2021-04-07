#!/usr/bin/env python

import setuptools

if __name__ == "__main__":
    setuptools.setup(
        name="min_creds_client",
        version="0.2.0",
        url="https://github.com/dlr-eoc/ukis-min_creds",
        author="German Aerospace Center (DLR)",
        author_email="ukis-helpdesk@dlr.de",
        license="Apache 2.0",
        description="client library for ukis-min_creds",
        python_requires='>=3.6',
        packages=setuptools.find_packages(),
        install_requires=[
            "requests>=0.2.3",
            "python-dateutil>=2.6.1"
        ]
    )
