# WebsiteMyMailchimp
Converts a mailchimp campaign to an html website hosted on an S3 bucket and Cloudfront distro of your choice.

### Purpose
The purpose of this program is to allow you to mirror your MailChimp newsletter to a website hosted on AWS S3. 
You will simply provide the top and bottom html templates and this program will plug in the latest mailchimp newsletter in between them.
Currently the newsletter gets inserted as a table element so the mobile adaptability is limited but can be addressed with the appropriate CSS.
Everytime you release a new mail newsletter you will run this program to update your s3 bucket in order to match the latest. The script
can be manually executed or easily automated with a task scheduler.

### What you will need
1. an html template called top.html (see templates/top.sample.html) containing all html for the top segment of the website
1. an html template called bottom.html (see templates/bottom.sample.html) containing all html for the bottom segment of the website
1. The url of your Mailchimp Newsletter in the format of https://us15.campaign-archive.com/home/?u=XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX 
1. an AWS S3 bucket
1. a Cloudfront Distro hosting the S3 bucket (todo: make optional)
1. awscli installed profile and credentials on the box the script is to be ran (todo: all explicit credential provider via config.ini)

### Copy over the templates or make your own
```bash
cp templates/top.sample.html templates/top.html
cp templates/bottom.sample.html templates/bottom.html
```

### Copy over the config.ini file and configure
```bash
cp config.sample.ini config.ini
```

### Configure your AWS Profile
```
aws configure
```

### Issues
1. Assumes you are hosting your s3 bucket from Cloudfront.
1. Currently relies on awscli configured profile containing the keys.
1. Currently the mailchimp images are hosted on thier own CDN. This results in slow load times or 400 error codes when loaded from the s3 endpoint. In order to solve for this, the images are scraped and uploaded by this program to your s3 bucket. Some basic javascript must run through and update the src attributes of all images to point to your s3 bucket.
```javascript
        const srcUrl = $(e).attr('src');
        if (srcUrl.includes("gallery.mailchimp.com") || srcUrl.includes("mcusercontent.com")) {
          const index = srcUrl.lastIndexOf("/") + 1;
          const filename = srcUrl.substr(index);
          $(e).attr('src', "https://mys3bucket.com/assets/mailchimpGallery/"+filename);
        }
      });
```

### Todo
1. provide AWS Lambda integration for easier automated updates
1. Allow for a config.ini AWS access key fallback

