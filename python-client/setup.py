#!/usr/bin/env python

import setuptools

if __name__ == "__main__":
    setuptools.setup(
        name="min_creds_client",
        version="0.1.1",
        python_requires='>=3.6',
        packages=setuptools.find_packages(),
        install_requires=[
            "requests>=0.2.3",
            "python-dateutil>=2.6.1"
        ]
    )